// Generates the Rust wire types of the t3code-contracts crate from the JSON Schema dump.
//
//   node gen-rust.mjs <rpc-schema.json> <out dir: packages/contracts/src/generated>
//
// Input is the dialect effect's toJsonSchemaMultiDocument emits (see dump.mts). Output is one Rust
// file per TypeScript source module plus `rpc_methods.rs` and `http_endpoints.rs`. Mapping rules:
// - Required key -> `T`; required `NullOr` -> `Option<T>` (always written, null when None);
//   optional key -> `Option<T>` omitted when None (an optional key that arrived as null decodes to None).
// - A named string definition is a branded id -> `Arc<str>` newtype. Named integers are aliases.
// - Unions decode by what the wire carries: a discriminator field (`_tag`, `type`, `kind`, ...), the
//   JSON kind of the value, or a key only one branch has. Tag-dispatched unions get an `Unknown`
//   variant so a value from a newer server still decodes. A branch's tag lives in its struct
//   (`#[serde(tag = ...)]`), so the struct carries it standalone and inside the union alike.
// - `serde_json::Value` appears only where the schema is arbitrary JSON (`{}`).
import fs from "node:fs";
import path from "node:path";

const [, , specPath, outDir] = process.argv;
if (!specPath || !outDir) throw new Error("usage: node gen-rust.mjs <rpc-schema.json> <out dir>");
const spec = JSON.parse(fs.readFileSync(specPath, "utf8"));

// ---------------------------------------------------------------------------------------------
// Names

const RUST_KEYWORDS = new Set(
  "as break const continue crate else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while async await dyn abstract become box do final macro override priv typeof unsized virtual yield try gen".split(
    " ",
  ),
);
const RESERVED_TYPES = new Set(["Option", "Value", "String", "Vec", "Box", "Result", "Some", "None", "Ok", "Err", "Self", "Map", "Arc"]);
const SYMBOL_NAMES = { "#": "Hash", "!": "Bang", "@": "At", "+": "Plus", "-": "Minus", "*": "Star", "/": "Slash", ".": "Dot", ":": "Colon", "~": "Tilde", "?": "Question", "=": "Equals", "<": "Lt", ">": "Gt" };
const words = (s) =>
  String(s)
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    .replace(/[^A-Za-z0-9]+/g, " ")
    .trim()
    .split(/\s+/)
    .filter(Boolean);
const pascal = (s) => {
  // Keep a name that is already PascalCase (e.g. `OrchestrationV2ThreadId`) as is.
  if (/^[A-Z][A-Za-z0-9]*$/.test(s) && !/^[A-Z0-9]+$/.test(s)) return s;
  return words(s)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase())
    .join("");
};
const snake = (s) => {
  let out = words(s).join("_").toLowerCase();
  if (out === "") out = "field";
  if (/^[0-9]/.test(out)) out = `n${out}`;
  return RUST_KEYWORDS.has(out) ? `r#${out}` : out;
};
const camelOfSnake = (s) => s.replace(/^r#/, "").replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
const typeIdent = (s) => {
  let out = pascal(s);
  if (out === "") out = Object.entries(SYMBOL_NAMES).reduce((acc, [k, v]) => acc + (String(s).includes(k) ? v : ""), "") || "Empty";
  if (/^[0-9]/.test(out)) out = `N${out}`;
  return RUST_KEYWORDS.has(out) || RESERVED_TYPES.has(out) ? `${out}Type` : out;
};
const variantIdent = (s) => {
  let out = typeIdent(s);
  return out === "SelfType" ? "SelfValue" : out;
};
const moduleIdent = (mod) => snake(mod).replace(/^r#/, "");

// ---------------------------------------------------------------------------------------------
// Definitions, including the inline RPC / HTTP roots, which get synthesized names.

const defs = { ...spec.definitions };
const defModule = { ...spec.definitionModules };
const isBare = (s) => s === null || s === undefined;
const synth = (name, schema, mod) => {
  if (isBare(schema) || schema.$ref || schema.$exportFailed) return schema;
  let n = typeIdent(name);
  for (let i = 2; n in defs; i++) n = `${typeIdent(name)}${i}`;
  defs[n] = schema;
  defModule[n] = mod;
  return { $ref: `#/definitions/${n}` };
};
const rpcName = (tag) => words(tag).map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join("");
for (const r of spec.rpc) {
  r.payload = isNotNullOnly(r.payload) ? r.payload : synth(`${rpcName(r.tag)}Payload`, r.payload, "rpc");
  r.success = synth(`${rpcName(r.tag)}Success`, r.success, "rpc");
  r.error = synth(`${rpcName(r.tag)}Error`, r.error, "rpc");
}
const httpModule = (api) => (api === "RelayApi" ? "relay" : "environmentHttp");
const httpName = (h) => `${pascal(h.group)}${pascal(h.name)}`;
for (const h of spec.http) {
  if (!h.clientCalls) continue;
  const base = httpName(h);
  const mod = httpModule(h.api);
  h.params = synth(`${base}Params`, h.params, mod);
  h.query = synth(`${base}Query`, h.query, mod);
  h.headers = synth(`${base}Headers`, h.headers, mod);
  for (const p of h.payload) p.schema = synth(`${base}Body`, p.schema, mod);
  for (const s of h.successes) s.schema = synth(`${base}Response`, s.schema, mod);
  const errs = h.errors.map((e) => e.schema).filter((e) => !isBare(e));
  h.errorSchema = errs.length === 0 ? null : synth(`${base}Error`, errs.length === 1 ? errs[0] : { anyOf: errs }, mod);
}
function isNotNullOnly(s) {
  return !isBare(s) && s.not?.type === "null" && Object.keys(s).length === 1;
}

const refName = (r) => r.replace("#/definitions/", "");
const resolve = (s) => (s?.$ref ? resolve(defs[refName(s.$ref)]) : s);
const isNull = (s) => s?.type === "null" && Object.keys(s).length === 1;
const isEmpty = (s) =>
  s === true || (typeof s === "object" && s !== null && Object.keys(s).filter((k) => k !== "description" && !k.startsWith("x-")).length === 0);

// allOf only appears as `[{type: object, additionalProperties: {}}]` next to a map; it adds nothing.
const normalize = (s) => {
  if (s && typeof s === "object" && Array.isArray(s.allOf) && s.allOf.every((a) => a.type === "object" && !a.properties)) {
    const { allOf: _allOf, ...rest } = s;
    return rest;
  }
  return s;
};

// Inherit a module for definitions the dump could not place: the module of a definition that uses them.
const refsOf = (s, out = new Set()) => {
  if (s && typeof s === "object") {
    if (typeof s.$ref === "string") out.add(refName(s.$ref));
    for (const v of Object.values(s)) refsOf(v, out);
  }
  return out;
};
const graph = new Map(Object.entries(defs).map(([n, s]) => [n, [...refsOf(s)].filter((x) => x in defs)]));
for (let changed = true; changed; ) {
  changed = false;
  for (const [n, targets] of [...graph].sort(([a], [b]) => a.localeCompare(b)))
    for (const t of targets)
      if (defModule[t] === undefined && defModule[n] !== undefined) {
        defModule[t] = defModule[n];
        changed = true;
      }
}
for (const n of Object.keys(defs)) defModule[n] ??= "baseSchemas";

// Reference cycles (Tarjan SCC): a direct reference inside a cycle needs a Box.
const sccOf = new Map();
{
  let index = 0;
  const idx = new Map(), low = new Map(), stack = [], on = new Set();
  const strong = (v) => {
    idx.set(v, index); low.set(v, index); index++; stack.push(v); on.add(v);
    for (const w of graph.get(v)) {
      if (!idx.has(w)) { strong(w); low.set(v, Math.min(low.get(v), low.get(w))); }
      else if (on.has(w)) low.set(v, Math.min(low.get(v), idx.get(w)));
    }
    if (low.get(v) === idx.get(v)) {
      const comp = [];
      let w;
      do { w = stack.pop(); on.delete(w); comp.push(w); } while (w !== v);
      const cyclic = comp.length > 1 || graph.get(v).includes(v);
      for (const c of comp) sccOf.set(c, cyclic ? v : null);
    }
  };
  for (const v of graph.keys()) if (!idx.has(v)) strong(v);
}
const inCycleWith = (a, b) => sccOf.get(a) != null && sccOf.get(a) === sccOf.get(b);

// ---------------------------------------------------------------------------------------------
// Union analysis

const nonNull = (alts) => alts.filter((a) => !isNull(a));
const altsOf = (s) => s.anyOf ?? s.oneOf;
const isStruct = (s) => {
  const r = normalize(resolve(s));
  return r?.type === "object" && r.properties !== undefined;
};
const isObjectUnion = (s) => {
  const r = resolve(s);
  const alts = r && altsOf(r);
  return Array.isArray(alts) && nonNull(alts).length === alts.length && alts.length > 1 && alts.every((a) => jsonKind(a) === "object");
};
const isLiteralEnum = (s) => Array.isArray(s?.enum) && s.enum.every((v) => typeof v === "string" || typeof v === "boolean");

/** The JSON kind a schema always produces, or "mixed"/"any". */
function jsonKind(s) {
  s = normalize(s);
  if (isEmpty(s)) return "any";
  if (s.$ref) return jsonKind(resolve(s));
  const alts = altsOf(s);
  if (alts) {
    const kinds = new Set(alts.map(jsonKind));
    return kinds.size === 1 ? [...kinds][0] : "mixed";
  }
  if (isNotNullOnly(s)) return "object";
  switch (s.type) {
    case "string": return "string";
    case "integer": case "number": return "number";
    case "boolean": return "boolean";
    case "array": return "array";
    case "object": return "object";
    case "null": return "null";
  }
  return "any";
}

const ABSENT = "absent";
const tagValue = (v) => (typeof v === "boolean" ? `b:${v}` : `s:${v}`);

/**
 * Discriminator values one union member can carry under `key`: { values: Set, open } or null when
 * the member's `key` is not a literal (so `key` cannot discriminate). `open` marks a plain-string
 * member that accepts any tag (a fallback branch).
 */
function tagSet(member, key) {
  if (isObjectUnion(member)) {
    const out = { values: new Set(), open: false };
    for (const m of altsOf(resolve(member))) {
      const t = tagSet(m, key);
      if (t === null) return null;
      for (const v of t.values) out.values.add(v);
      out.open ||= t.open;
    }
    return out;
  }
  if (!isStruct(member)) return null;
  const r = normalize(resolve(member));
  const prop = r.properties[key];
  const required = (r.required ?? []).includes(key);
  if (prop === undefined) return { values: new Set([ABSENT]), open: false };
  let p = resolve(prop);
  if (isNull(p)) return { values: new Set([ABSENT]), open: false };
  let nullable = false;
  const pAlts = p && altsOf(p);
  if (pAlts) {
    const nn = nonNull(pAlts);
    nullable = nn.length < pAlts.length;
    if (nn.length === 0) return { values: new Set([ABSENT]), open: false };
    if (nn.length !== 1) return null;
    p = resolve(nn[0]);
  }
  const values = new Set();
  if (!required || nullable) values.add(ABSENT);
  if (isLiteralEnum(p)) {
    for (const v of p.enum) values.add(tagValue(v));
    return { values, open: false };
  }
  if (p?.type === "string" && !p.enum) return { values, open: true };
  return null;
}

/** Members of an object union that are structurally one branch except for a single property. */
function mergeSameTag(a, b) {
  const ra = normalize(resolve(a)), rb = normalize(resolve(b));
  if (!isStruct(ra) || !isStruct(rb)) return null;
  const ka = Object.keys(ra.properties), kb = Object.keys(rb.properties);
  if (ka.length !== kb.length || ka.some((k) => !(k in rb.properties))) return null;
  if (JSON.stringify([...(ra.required ?? [])].sort()) !== JSON.stringify([...(rb.required ?? [])].sort())) return null;
  const differing = ka.filter((k) => JSON.stringify(ra.properties[k]) !== JSON.stringify(rb.properties[k]));
  if (differing.length !== 1) return null;
  const k = differing[0];
  return { ...ra, properties: { ...ra.properties, [k]: { anyOf: [ra.properties[k], rb.properties[k]] } } };
}

/** Plans how an object union decodes: by a tag key, by key presence, or (last resort) by trial. */
function planObjectUnion(members) {
  const candidateKeys = new Set();
  const collectKeys = (m) => {
    if (isObjectUnion(m)) for (const x of altsOf(resolve(m))) collectKeys(x);
    else if (isStruct(m)) for (const k of Object.keys(normalize(resolve(m)).properties)) candidateKeys.add(k);
  };
  members.forEach(collectKeys);
  const preferred = ["_tag", "type", "kind"];
  const ordered = [...preferred.filter((k) => candidateKeys.has(k)), ...[...candidateKeys].filter((k) => !preferred.includes(k)).sort()];
  let overlapping;
  for (const key of ordered) {
    let ms = members.map((m) => ({ schema: m, tags: tagSet(m, key) }));
    if (ms.some((m) => m.tags === null)) continue;
    // Two closed members with the same tags may be one branch whose single field differs.
    for (let i = 0; i < ms.length; i++)
      for (let j = i + 1; j < ms.length; j++) {
        const a = ms[i], b = ms[j];
        if (a.tags.open || b.tags.open || a.tags.values.size !== b.tags.values.size) continue;
        if ([...a.tags.values].some((v) => !b.tags.values.has(v))) continue;
        const merged = mergeSameTag(a.schema, b.schema);
        if (merged) {
          ms[i] = { schema: merged, tags: a.tags };
          ms.splice(j, 1);
          j--;
        }
      }
    const open = ms.filter((m) => m.tags.open);
    if (open.length > 1) continue;
    const seen = new Set();
    let disjoint = true;
    for (const m of ms) {
      if (m.tags.open) continue;
      for (const v of m.tags.values) {
        if (seen.has(v)) disjoint = false;
        seen.add(v);
      }
    }
    if (ms.filter((m) => !m.tags.open).every((m) => m.tags.values.size === 1 && m.tags.values.has(ABSENT))) continue;
    const plan = { kind: "tag", key, members: ms.filter((m) => !m.tags.open), fallback: open[0]?.schema };
    if (disjoint) return plan;
    // Members sharing a tag are tried in declaration order, as the TypeScript union does.
    overlapping ??= plan;
  }
  if (overlapping) return overlapping;
  // A key only one member has picks that member; at most one member may lack such a key.
  const structs = members.map((m) => (isStruct(m) ? normalize(resolve(m)) : null));
  if (structs.every(Boolean)) {
    const marks = structs.map((s, i) =>
      (s.required ?? []).find((k) => structs.every((o, j) => j === i || !(k in o.properties))),
    );
    if (marks.filter((m) => m === undefined).length <= 1)
      return { kind: "presence", members: members.map((schema, i) => ({ schema, key: marks[i] })) };
  }
  return { kind: "trial", members: members.map((schema) => ({ schema })) };
}

// Struct tags: a struct whose discriminator is one required string literal writes it itself.
const structTagKey = new Map(); // definition name -> key
const tagKeyCandidates = new Map(); // definition name -> keys of unions it is a member of
const noteUnionKeys = (s) => {
  if (!s || typeof s !== "object") return;
  const alts = altsOf(s);
  if (Array.isArray(alts)) {
    const nn = nonNull(alts);
    if (nn.length > 1 && nn.every((a) => jsonKind(a) === "object")) {
      const plan = planObjectUnion(nn);
      if (plan.kind === "tag")
        for (const m of plan.members)
          if (m.schema.$ref) {
            const n = refName(m.schema.$ref);
            if (!tagKeyCandidates.has(n)) tagKeyCandidates.set(n, []);
            tagKeyCandidates.get(n).push(plan.key);
          }
    }
  }
  for (const v of Object.values(s)) noteUnionKeys(v);
};
for (const s of Object.values(defs)) noteUnionKeys(s);
const singleLiteral = (struct, key) => {
  const p = resolve(struct.properties[key]);
  return (struct.required ?? []).includes(key) && Array.isArray(p?.enum) && p.enum.length === 1 && typeof p.enum[0] === "string" && p.type === "string"
    ? p.enum[0]
    : undefined;
};
for (const [name, s0] of Object.entries(defs)) {
  const s = normalize(s0);
  if (!isStruct(s) || s.$ref) continue;
  const keys = ["_tag", ...(tagKeyCandidates.get(name) ?? [])];
  const key = keys.find((k) => k in s.properties && singleLiteral(s, k) !== undefined);
  if (key !== undefined) structTagKey.set(name, key);
}

// ---------------------------------------------------------------------------------------------
// Emission

const VALUE = "::serde_json::Value";
const JSON_MAP = "::serde_json::Map<String, ::serde_json::Value>";
const items = new Map(); // module -> Rust source chunks
const emit = (mod, code) => {
  if (!items.has(mod)) items.set(mod, []);
  items.get(mod).push(code);
};
const stats = { structs: 0, ids: 0, stringEnums: 0, openEnums: 0, tagUnions: 0, kindUnions: 0, presenceUnions: 0, trialUnions: 0, aliases: 0, forwardCompatFields: 0, valueSites: 0 };
const valueSites = [];
const trialSites = [];
const used = new Set();
const named = new Map(); // definition name -> Rust type name (once emitted)
const inlineByShape = new Map(); // canonical inline JSON -> Rust type name
const fresh = (base) => {
  let n = typeIdent(base);
  if (!used.has(n)) { used.add(n); return n; }
  for (let i = 2; ; i++) if (!used.has(`${n}${i}`)) { used.add(`${n}${i}`); return `${n}${i}`; }
};
const canon = (s) => JSON.stringify(s, (k, v) => (k === "description" ? undefined : v));
for (const n of Object.keys(defs)) used.add(typeIdent(n));
const shapeToDef = new Map();
for (const [n, s] of Object.entries(defs).sort(([a], [b]) => a.localeCompare(b))) if (!shapeToDef.has(canon(s))) shapeToDef.set(canon(s), n);

const docLines = (text, indent = "") => (text ? String(text).split("\n").map((l) => `${indent}/// ${l}`.trimEnd()).join("\n") + "\n" : "");
const DERIVE = "#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]";

/**
 * Rust type for schema `s`. ctx: { owner: enclosing definition (cycle check), module, hint: name
 * for inline types, direct: false under a Vec/map (no Box needed there) }.
 */
function rustType(s, ctx0) {
  // Nested types never inherit the enclosing definition's own name, doc or struct tag.
  const ctx = { owner: ctx0.owner, module: ctx0.module, hint: ctx0.hint, direct: ctx0.direct };
  s = normalize(s);
  if (s === undefined || isEmpty(s)) return value(ctx, "arbitrary JSON");
  if (s.$exportFailed) throw new Error(`export failed at ${ctx.hint}`);
  if (s.$ref) {
    const target = refName(s.$ref);
    const name = ensureDef(target);
    return ctx.direct !== false && inCycleWith(ctx.owner, target) ? `Box<${name}>` : name;
  }
  if (isNotNullOnly(s)) return "crate::wire::Empty";
  const alts = altsOf(s);
  if (alts) {
    const nn = nonNull(alts);
    if (nn.length === 0) return "()";
    if (nn.length < alts.length) return `Option<${rustType(nn.length === 1 ? nn[0] : { anyOf: nn }, ctx)}>`;
    if (nn.length === 1) return rustType(nn[0], ctx);
    return inline(s, ctx, (name, c) => emitUnion(name, s, nn, c));
  }
  if (Array.isArray(s.type)) return value(ctx, "type list");
  switch (s.type) {
    case "string":
      return s.enum ? inline(s, ctx, (name, c) => emitStringEnum(name, s.enum, c, s.description)) : "String";
    case "integer":
      return integerType(s);
    case "number":
      return "f64";
    case "boolean":
      if (Array.isArray(s.enum) && s.enum.length === 1) return s.enum[0] ? "crate::wire::True" : "crate::wire::False";
      return "bool";
    case "null":
      return "()";
    case "array": {
      const item = s.items ?? s.prefixItems?.[0];
      if (Array.isArray(s.prefixItems) && !(s.items && s.prefixItems.every((p) => canon(p) === canon(s.items)))) {
        if (s.items) return value(ctx, "tuple with rest");
        return `(${s.prefixItems.map((p, i) => rustType(p, { ...ctx, hint: `${ctx.hint}${i}` })).join(", ")},)`;
      }
      return `Vec<${rustType(item, { ...ctx, hint: `${ctx.hint}Item`, direct: false })}>`;
    }
    case "object": {
      if (s.properties && Object.keys(s.properties).length > 0) return inline(s, ctx, (name, c) => emitStruct(name, s, c));
      if (s.additionalProperties === false || s.properties) return "crate::wire::Empty";
      const extra = s.additionalProperties ?? Object.values(s.patternProperties ?? {})[0];
      if (extra && extra !== true && !isEmpty(extra))
        return `::std::collections::BTreeMap<String, ${rustType(extra, { ...ctx, hint: `${ctx.hint}Value`, direct: false })}>`;
      stats.valueSites++;
      valueSites.push(`${ctx.hint}: object with arbitrary values`);
      return JSON_MAP;
    }
  }
  return value(ctx, `unhandled ${JSON.stringify(s).slice(0, 60)}`);
}
function value(ctx, why) {
  stats.valueSites++;
  valueSites.push(`${ctx.hint}: ${why}`);
  return VALUE;
}
function integerType(s) {
  return typeof s.minimum === "number" && s.minimum >= 0 ? "u64" : typeof s.exclusiveMinimum === "number" && s.exclusiveMinimum >= -1 ? "u64" : "i64";
}

/** Names an inline schema: reuses a definition or an earlier inline type of the same shape. */
function inline(s, ctx, emitFn) {
  const key = canon(s);
  const asDef = shapeToDef.get(key);
  if (asDef !== undefined) return ensureDef(asDef);
  if (inlineByShape.has(key)) return inlineByShape.get(key);
  const name = fresh(ctx.hint);
  inlineByShape.set(key, name);
  emitFn(name, { owner: ctx.owner, module: ctx.module, hint: name });
  return name;
}

/** Emits definition `n` once and returns its Rust name. */
function ensureDef(n) {
  if (named.has(n)) return named.get(n);
  const name = typeIdent(n);
  named.set(n, name);
  const s = normalize(defs[n]);
  const ctx = { owner: n, module: defModule[n], hint: name, selfDef: n, doc: `\`${n}\` from \`${defModule[n]}.ts\`.` };
  emitNamed(name, s, ctx);
  return name;
}

function emitNamed(name, s, ctx) {
  const doc = docLines(s.description) + docLines(ctx.doc);
  if (s.$ref) {
    stats.aliases++;
    emit(ctx.module, `${doc}pub type ${name} = ${rustType(s, { ...ctx, selfDef: undefined })};\n`);
    return;
  }
  const alts = altsOf(s);
  if (alts) {
    const nn = nonNull(alts);
    if (nn.length !== alts.length || nn.length === 1) {
      stats.aliases++;
      const inner = nn.length === 1 ? nn[0] : { anyOf: nn };
      const t = rustType(inner, { ...ctx, hint: `${name}Value`, selfDef: undefined });
      emit(ctx.module, `${doc}pub type ${name} = ${nn.length === alts.length ? t : `Option<${t}>`};\n`);
      return;
    }
    emitUnion(name, s, nn, ctx);
    return;
  }
  if (s.type === "string" && s.enum) return emitStringEnum(name, s.enum, ctx, s.description);
  if (s.type === "string") {
    stats.ids++;
    emit(ctx.module, `crate::wire::id_newtype! {\n${doc}    ${name}\n}\n`);
    return;
  }
  if (s.type === "object" && s.properties) return emitStruct(name, s, ctx);
  stats.aliases++;
  const t = rustType(s, { ...ctx, hint: `${name}Inner`, selfDef: undefined });
  emit(ctx.module, `${doc}pub type ${name} = ${t};\n`);
}

function emitStringEnum(name, values, ctx, description) {
  if (values.some((v) => typeof v !== "string")) {
    // Mixed literal enums do not occur in the contracts today.
    stats.aliases++;
    emit(ctx.module, `${docLines(description)}pub type ${name} = ${value(ctx, "non-string enum")};\n`);
    return;
  }
  stats.stringEnums++;
  const seen = new Set();
  const variants = values.map((v) => {
    let ident = variantIdent(v === "" ? "Empty" : v);
    while (seen.has(ident)) ident = `${ident}Alt`;
    seen.add(ident);
    return `    ${ident} = ${JSON.stringify(v)},`;
  });
  const doc = docLines(description, "    ") + docLines(ctx.doc ?? `Inline enum at \`${ctx.hint}\`.`, "    ");
  emit(ctx.module, `crate::wire::string_enum! {\n${doc}    ${name} {\n${variants.map((v) => `    ${v}`).join("\n")}\n    }\n}\n`);
}

function isForwardCompatible(prop) {
  if (prop?.["x-forward-compatible"]) return true;
  const alts = prop && altsOf(prop);
  return Array.isArray(alts) && alts.some((a) => a?.["x-forward-compatible"] && !a.$ref && !(a.type === "object" && a.properties));
}

function emitStruct(name, s, ctx, tagKeyOverride) {
  stats.structs++;
  const required = new Set(s.required ?? []);
  const ownTag = tagKeyOverride ?? (ctx.selfDef ? structTagKey.get(ctx.selfDef) : undefined);
  const tagLiteral = ownTag !== undefined ? singleLiteral(s, ownTag) : undefined;
  const seen = new Set();
  let allOptional = true;
  const fields = [];
  for (const [key, prop0] of Object.entries(s.properties)) {
    if (tagLiteral !== undefined && key === ownTag) continue;
    const prop = normalize(prop0);
    let field = snake(key);
    while (seen.has(field)) field = `${field}_`;
    seen.add(field);
    let ty = rustType(prop, { owner: ctx.owner, module: ctx.module, hint: `${name}${pascal(key)}` });
    const attrs = [];
    if (camelOfSnake(field) !== key) attrs.push(`rename = ${JSON.stringify(key)}`);
    const forwardCompat = isForwardCompatible(prop);
    if (!required.has(key)) {
      if (!ty.startsWith("Option<")) ty = `Option<${ty}>`;
      attrs.push("default", `skip_serializing_if = "Option::is_none"`);
    }
    if (forwardCompat) {
      // A value this client cannot parse decodes to None instead of failing the whole message.
      if (!ty.startsWith("Option<")) ty = `Option<${ty}>`;
      if (!attrs.includes("default")) attrs.push("default");
      attrs.push(`deserialize_with = "crate::wire::lenient"`);
      stats.forwardCompatFields++;
    }
    if (!ty.startsWith("Option<")) allOptional = false;
    fields.push(`${docLines(prop.description, "    ")}${attrs.length ? `    #[serde(${attrs.join(", ")})]\n` : ""}    pub ${field}: ${ty},`);
  }
  const containerAttrs = [`rename_all = "camelCase"`];
  if (tagLiteral !== undefined) containerAttrs.unshift(`tag = ${JSON.stringify(ownTag)}`, `rename = ${JSON.stringify(tagLiteral)}`);
  const derive = allOptional && tagLiteral === undefined
    ? "#[derive(Debug, Clone, Default, PartialEq, ::serde::Serialize, ::serde::Deserialize)]"
    : DERIVE;
  const marker = s["x-record-id-table"] ? "/// May arrive with an id table; decode it with `crate::orchestration_v2_id_table`.\n" : "";
  const tagDoc = tagLiteral !== undefined ? `/// Wire tag: \`${ownTag}: ${JSON.stringify(tagLiteral)}\`.\n` : "";
  emit(
    ctx.module,
    `${docLines(s.description)}${docLines(ctx.doc ?? `Inline object at \`${ctx.hint}\`.`)}${tagDoc}${marker}${derive}\n#[serde(${containerAttrs.join(", ")})]\npub struct ${name} {\n${fields.join("\n")}${fields.length ? "\n" : ""}}\n`,
  );
  return name;
}

/** Variant name for a union member. */
function memberIdent(m, key, unionName) {
  if (m.schema.$ref) {
    const n = typeIdent(refName(m.schema.$ref));
    // Drop the union's own name as a prefix: `TerminalError::CwdError` reads better than `TerminalError::TerminalCwdError`.
    return n.startsWith(unionName) && n.length > unionName.length && /^[A-Z]/.test(n.slice(unionName.length)) ? n.slice(unionName.length) : n;
  }
  const vals = [...(m.tags?.values ?? [])].filter((v) => v !== ABSENT);
  if (vals.length === 0) return `No${pascal(key)}`;
  return vals
    .map((v) => (v === "b:true" ? pascal(key) : v === "b:false" ? `Not${pascal(key)}` : variantIdent(v.slice(2))))
    .join("Or");
}

function emitUnion(name, s, alts, ctx) {
  const kinds = alts.map(jsonKind);
  const doc = docLines(s.description) + docLines(ctx.doc ?? `Inline union at \`${ctx.hint}\`.`);
  const sub = (schema, hint) => rustType(schema, { owner: ctx.owner, module: ctx.module, hint });

  // All strings: one enum, open when a plain string is among the members.
  if (kinds.every((k) => k === "string")) {
    const closed = alts.map(resolve).filter((a) => Array.isArray(a.enum));
    const values = [...new Set(closed.flatMap((a) => a.enum))];
    if (closed.length === alts.length) return emitStringEnum(name, values, ctx, s.description);
    stats.openEnums++;
    const known = inline({ type: "string", enum: values }, { ...ctx, hint: `${name}Known` }, (n, c) => emitStringEnum(n, values, c));
    emit(
      ctx.module,
      `${doc}/// A known value, or any other string.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize)]\n#[serde(untagged)]\npub enum ${name} {\n    Known(${known}),\n    Other(String),\n}\nimpl<'de> ::serde::Deserialize<'de> for ${name} {\n    fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n        let s = String::deserialize(d)?;\n        Ok(match ${known}::from_wire(&s) {\n            Some(k) => Self::Known(k),\n            None => Self::Other(s),\n        })\n    }\n}\n`,
    );
    return;
  }

  // Mixed JSON kinds: dispatch on the kind; objects among them form their own union.
  if (kinds.some((k) => k !== "object")) {
    const byKind = new Map();
    alts.forEach((a, i) => {
      const k = kinds[i];
      if (!byKind.has(k)) byKind.set(k, []);
      byKind.get(k).push(a);
    });
    if (![...byKind.keys()].some((k) => k === "mixed" || k === "any")) {
      stats.kindUnions++;
      const variants = [];
      for (const [k, members] of byKind) {
        const schema = members.length === 1 ? members[0] : { anyOf: members };
        const hint = `${name}${{ string: "Text", number: "Number", boolean: "Bool", array: "List", object: "Object" }[k]}`;
        const ty = sub(schema, hint);
        const ident = members.length === 1 && members[0].$ref ? typeIdent(refName(members[0].$ref)) : { string: "Text", number: ty === "f64" ? "Number" : "Int", boolean: "Bool", array: "List", object: ty.startsWith("::std::collections::BTreeMap") || ty === JSON_MAP ? "Map" : "Object" }[k];
        variants.push({ k, ident, ty });
      }
      const kindName = { string: "String", number: "Number", boolean: "Bool", array: "Array", object: "Object" };
      emit(
        ctx.module,
        `${doc}#[derive(Debug, Clone, PartialEq, ::serde::Serialize)]\n#[serde(untagged)]\npub enum ${name} {\n${variants.map((v) => `    ${v.ident}(${v.ty}),`).join("\n")}\n}\nimpl<'de> ::serde::Deserialize<'de> for ${name} {\n    fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n        let raw = crate::wire::Raw::new(d)?;\n        Ok(match raw.kind() {\n${variants.map((v) => `            crate::wire::JsonKind::${kindName[v.k]} => Self::${v.ident}(raw.decode()?),`).join("\n")}\n            other => return Err(raw.unexpected(other, ${JSON.stringify(name)})),\n        })\n    }\n}\n`,
      );
      return;
    }
  }

  // Objects.
  const plan = kinds.every((k) => k === "object") ? planObjectUnion(alts) : { kind: "trial", members: alts.map((schema) => ({ schema })) };
  const seen = new Set();
  const identFor = (m) => {
    let ident = memberIdent(m, plan.key, name);
    if (ident === "Unknown") ident = "UnknownValue";
    if (seen.has(ident) && isStruct(m.schema)) {
      // Members that share a tag: name this one by its other literal fields.
      const r = normalize(resolve(m.schema));
      const extra = Object.keys(r.properties)
        .filter((k) => k !== plan.key)
        .map((k) => resolve(r.properties[k]))
        .filter((p) => Array.isArray(p?.enum) && p.enum.length === 1 && typeof p.enum[0] === "string")
        .map((p) => variantIdent(p.enum[0]))
        .join("");
      if (extra) ident = `${ident}${extra}`;
    }
    while (seen.has(ident)) ident = `${ident}Alt`;
    seen.add(ident);
    return ident;
  };
  const memberType = (m, ident) => {
    if (m.schema.$ref) return sub(m.schema, `${name}${ident}`);
    const r = normalize(m.schema);
    if (isStruct(r)) {
      const tagKey = plan.kind === "tag" && singleLiteral(r, plan.key) !== undefined ? plan.key : undefined;
      return inline(r, { owner: ctx.owner, module: ctx.module, hint: `${name}${ident}` }, (n, c) => emitStruct(n, r, c, tagKey));
    }
    return sub(r, `${name}${ident}`);
  };

  if (plan.kind === "tag") {
    // A fallback holding only the tag (and arbitrary extras) is the `Unknown` variant itself.
    let fallbackType = VALUE;
    if (plan.fallback) {
      const r = normalize(resolve(plan.fallback));
      const req = (r.required ?? []).filter((k) => k !== plan.key);
      if (req.length > 0) fallbackType = memberType({ schema: plan.fallback }, "Unknown");
    }
    // Only one real member left, itself a union with its own Unknown: that union is the type.
    if (plan.members.length === 1 && isObjectUnion(plan.members[0].schema) && plan.members[0].schema.$ref && fallbackType === VALUE) {
      stats.aliases++;
      emit(ctx.module, `${doc}pub type ${name} = ${sub(plan.members[0].schema, name)};\n`);
      return;
    }
    stats.tagUnions++;
    const variants = plan.members.map((m) => {
      const ident = identFor(m);
      return { ident, ty: memberType(m, ident), values: [...m.tags.values] };
    });
    // Tag value -> variants carrying it, in declaration order.
    const byValue = new Map();
    for (const v of variants)
      for (const x of v.values) {
        if (!byValue.has(x)) byValue.set(x, []);
        byValue.get(x).push(v.ident);
      }
    const decodeFirst = (idents) =>
      idents.length === 1
        ? `Self::${idents[0]}(raw.decode()?)`
        : idents.slice(0, -1).map((i) => `if let Ok(v) = raw.decode() { Self::${i}(v) } else `).join("") + `{ Self::${idents.at(-1)}(raw.decode()?) }`;
    // Group string values that lead to the same variants into one arm.
    const strGroups = new Map();
    for (const [x, idents] of byValue)
      if (x.startsWith("s:")) {
        const k = idents.join(",");
        if (!strGroups.has(k)) strGroups.set(k, { idents, values: [] });
        strGroups.get(k).values.push(JSON.stringify(x.slice(2)));
      }
    const strArms = [...strGroups.values()].map((g) => `                ${g.values.join(" | ")} => ${decodeFirst(g.idents)},`);
    const otherArms = [];
    for (const [x, idents] of byValue) {
      if (x === "b:true" || x === "b:false") otherArms.push(`            crate::wire::Tag::Bool(${x.slice(2)}) => ${decodeFirst(idents)},`);
      if (x === ABSENT) otherArms.push(`            crate::wire::Tag::Absent => ${decodeFirst(idents)},`);
    }
    const unknownArm = "Self::Unknown(raw.decode()?)";
    const strMatch = strArms.length
      ? `            crate::wire::Tag::Str(tag) => match &*tag {\n${strArms.join("\n")}\n                _ => ${unknownArm},\n            },\n`
      : "";
    emit(
      ctx.module,
      `${doc}/// Decoded by its \`${plan.key}\` field; a tag this client does not know becomes \`Unknown\`.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize)]\n#[serde(untagged)]\npub enum ${name} {\n${variants.map((v) => `    ${v.ident}(${v.ty}),`).join("\n")}\n    /// A value from a newer server.\n    Unknown(${fallbackType}),\n}\nimpl<'de> ::serde::Deserialize<'de> for ${name} {\n    fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n        let raw = crate::wire::Raw::new(d)?;\n        Ok(match raw.tag(${JSON.stringify(plan.key)})? {\n${strMatch}${otherArms.join("\n")}${otherArms.length ? "\n" : ""}            _ => ${unknownArm},\n        })\n    }\n}\n`,
    );
    return;
  }

  const variants = plan.members.map((m) => {
    const ident = identFor(m);
    return { ident, ty: memberType(m, ident), key: m.key };
  });
  if (plan.kind === "presence") {
    stats.presenceUnions++;
    const withKey = variants.filter((v) => v.key !== undefined);
    const rest = variants.find((v) => v.key === undefined);
    const body = withKey.map((v, i) => `${i === 0 ? "        Ok(if" : " else if"} raw.has_key(${JSON.stringify(v.key)})? {\n            Self::${v.ident}(raw.decode()?)\n        }`).join("");
    const tail = rest
      ? ` else {\n            Self::${rest.ident}(raw.decode()?)\n        })`
      : ` else {\n            return Err(raw.unexpected(raw.kind(), ${JSON.stringify(name)}));\n        })`;
    emit(
      ctx.module,
      `${doc}/// Decoded by which keys the object carries.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize)]\n#[serde(untagged)]\npub enum ${name} {\n${variants.map((v) => `    ${v.ident}(${v.ty}),`).join("\n")}\n}\nimpl<'de> ::serde::Deserialize<'de> for ${name} {\n    fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n        let raw = crate::wire::Raw::new(d)?;\n${body}${tail}\n    }\n}\n`,
    );
    return;
  }
  stats.trialUnions++;
  trialSites.push(name);
  emit(
    ctx.module,
    `${doc}/// No field tells the branches apart; the first branch that decodes wins.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize)]\n#[serde(untagged)]\npub enum ${name} {\n${variants.map((v) => `    ${v.ident}(${v.ty}),`).join("\n")}\n}\nimpl<'de> ::serde::Deserialize<'de> for ${name} {\n    fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n        let raw = crate::wire::Raw::new(d)?;\n${variants.map((v) => `        if let Ok(v) = raw.decode() {\n            return Ok(Self::${v.ident}(v));\n        }`).join("\n")}\n        Err(raw.unexpected(raw.kind(), ${JSON.stringify(name)}))\n    }\n}\n`,
  );
}

// ---------------------------------------------------------------------------------------------
// Drive: every definition, then the RPC and HTTP descriptors.

for (const n of Object.keys(defs).sort((a, b) => a.localeCompare(b))) ensureDef(n);

// Method and endpoint types share names with payload types (`DeviceDetail`), so root types are
// written from the crate root.
const rootType = (s, ifNone) => {
  if (isBare(s)) return ifNone;
  if (isNotNullOnly(s)) return "crate::wire::Empty";
  const t = rustType(s, { owner: "", module: "rpc", hint: "Root" });
  return t.replace(/(?<![:\w])([A-Z]\w*)/g, (m) => (["Option", "Vec", "Box", "String"].includes(m) ? m : `crate::${m}`));
};
const scopeVariant = (scope) => variantIdent(scope);
ensureDef("AuthEnvironmentScope");
const methodIdents = new Set();
const rpcLines = spec.rpc.map((r) => {
  let ident = rpcName(r.tag);
  while (methodIdents.has(ident)) ident = `${ident}Alt`;
  methodIdents.add(ident);
  return `    /// \`${r.tag}\`${r.stream ? " (stream)" : ""}.\n    ${ident} {\n        tag: ${JSON.stringify(r.tag)},\n        scope: ${scopeVariant(r.scope)},\n        client_guarded: ${r.clientGuarded},\n        payload: ${rootType(r.payload, "()")},\n        success: ${rootType(r.success, "()")},\n        error: ${rootType(r.error, "crate::wire::Never")},\n        stream: ${r.stream},\n    }`;
});
const rpcMethods = `// @generated by scripts/contracts/gen-rust.mjs. Do not edit.
//! One zero-sized type per RPC method, implementing [\`crate::rpc::RpcMethod\`].

crate::rpc::rpc_methods! {
${rpcLines.join("\n")}
}
`;

const methodVariant = (m) => pascal(m.toLowerCase());
const httpLines = spec.http
  .filter((h) => h.clientCalls)
  .map((h) => {
    const json = h.payload.find((p) => p.encoding === "Json");
    const form = h.payload.find((p) => p.encoding === "FormUrlEncoded");
    const bytes = h.payload.find((p) => p.encoding === "Uint8Array");
    const body = json ?? form ?? bytes;
    const bodyKind = json ? "Json" : form ? "Form" : bytes ? "Bytes" : "None";
    const bodyType = bytes && !json && !form ? "Vec<u8>" : rootType(body?.schema, "()");
    const ok = h.successes[0];
    return `    /// \`${h.method} ${h.path}\` (${h.api} \`${h.group}.${h.name}\`).\n    ${httpName(h)} {\n        method: ${methodVariant(h.method)},\n        path: ${JSON.stringify(h.path)},\n        params: ${rootType(h.params, "()")},\n        query: ${rootType(h.query, "()")},\n        headers: ${rootType(h.headers, "()")},\n        body: ${bodyKind}(${bodyType}),\n        success: ${ok.status} => ${rootType(ok.schema, "()")},\n        error: ${rootType(h.errorSchema, "crate::wire::Never")},\n    }`;
  });
const httpEndpoints = `// @generated by scripts/contracts/gen-rust.mjs. Do not edit.
//! One zero-sized type per HTTP endpoint that client code calls, implementing [\`crate::http::HttpEndpoint\`].

crate::http::http_endpoints! {
${httpLines.join("\n")}
}
`;

// ---------------------------------------------------------------------------------------------
// Write files

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });
const header = (mod) => `// @generated by scripts/contracts/gen-rust.mjs from schema/rpc-schema.json. Do not edit.
//! Wire types from \`packages/contracts/src/${mod}.ts\`.
#![allow(unused_imports)]
use super::*;

`;
const modules = [...items.keys()].sort();
for (const mod of modules) fs.writeFileSync(path.join(outDir, `${moduleIdent(mod)}.rs`), header(mod) + items.get(mod).join("\n"));
fs.writeFileSync(path.join(outDir, "rpc_methods.rs"), rpcMethods);
fs.writeFileSync(path.join(outDir, "http_endpoints.rs"), httpEndpoints);
fs.writeFileSync(
  path.join(outDir, "mod.rs"),
  `// @generated by scripts/contracts/gen-rust.mjs. Do not edit.
//! Generated wire types, one module per TypeScript source file. Every type is re-exported here.
#![allow(clippy::large_enum_variant, clippy::enum_variant_names)]

${modules.map((m) => `mod ${moduleIdent(m)};\npub use ${moduleIdent(m)}::*;`).join("\n")}
// rpc_methods.rs and http_endpoints.rs are included by crate::rpc and crate::http.
`,
);

const report = {
  definitions: Object.keys(defs).length,
  modules: modules.length,
  rpcMethods: spec.rpc.length,
  httpEndpoints: httpLines.length,
  ...stats,
  trialSites,
  valueSites,
};
console.log(JSON.stringify(report, null, 2));
