// Dumps every T3 Code wire contract as JSON Schema (draft 2020-12, encoded/wire side).
// Run through regen.sh, which runs this from <checkout>/packages/contracts so effect resolves there:
//   node dump.mts <outFile>
//
// Wire side: RpcSerialization.layerJson and HttpApi JSON bodies both encode with
// Schema.toCodecJson, and Schema.toJsonSchemaDocument / SchemaRepresentation.toJsonSchemaMultiDocument
// describe exactly the encoded side of that codec. Query/params/headers use toCodecStringTree.
//
// Besides the schemas, the dump records what the Rust generator needs and cannot see in JSON Schema:
// each RPC's required scope and client guard, each definition's source module, and which HTTP
// endpoints client code calls.
import * as path from "node:path";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
// Must load before effect/contracts: registers the in-memory source patches (brands, forward-compatible fields).
import { FORWARD_COMPAT_KEY, ID_TABLE_KEY, assertAllApplied, installPatches } from "./hooks.mts";
installPatches({ idTable: true });

const cwd = process.cwd();
const outFile = process.argv[2];
if (!outFile) throw new Error("usage: node dump.mts <outFile>");
const repo = path.resolve(cwd, "../..");
const imp = (rel: string) => import(pathToFileURL(path.join(cwd, rel)).href);
// Bare specifiers resolve from this script's directory, so load effect from the checkout's package instead.
const requireFromCwd = createRequire(path.join(cwd, "package.json"));
const effect = <T,>(spec: string): Promise<T> => import(pathToFileURL(requireFromCwd.resolve(spec)).href);

const Schema = await effect<typeof import("effect/Schema")>("effect/Schema");
const SchemaAST = await effect<typeof import("effect/SchemaAST")>("effect/SchemaAST");
const SchemaRepresentation = await effect<typeof import("effect/SchemaRepresentation")>(
  "effect/SchemaRepresentation",
);
const RpcSchema = await effect<typeof import("effect/rpc/RpcSchema")>("effect/rpc/RpcSchema");
const HttpApi = await effect<typeof import("effect/http-api/HttpApi")>("effect/http-api/HttpApi");

const contracts: Record<string, unknown> = await imp("src/index.ts");
const relay: Record<string, unknown> = await imp("src/relay.ts");
assertAllApplied();

type AnySchema = import("effect/Schema").Top;
type AST = import("effect/SchemaAST").AST;
const isSchema = (u: unknown): u is AnySchema => Schema.isSchema(u);

// --- 1. Name table: exported const names for schemas that carry no identifier annotation. ---
// An AST shared by several exports (aliases) gets no export name, so one export's name
// never leaks onto another type. Plain keywords (string, number...) are never named.
const exactNames = new Map<AST, Set<string>>();
const encodedNames = new Map<AST, Set<string>>();
const note = (m: Map<AST, Set<string>>, ast: AST, name: string) => {
  const set = m.get(ast) ?? new Set();
  set.add(name);
  m.set(ast, set);
};
const isBareKeyword = (ast: AST) =>
  ["String", "Number", "Boolean", "Null", "Unknown", "Any", "Void", "Undefined", "Never", "BigInt"].includes(ast._tag) &&
  !ast.checks?.length;
for (const mod of [contracts, relay]) {
  for (const [name, value] of Object.entries(mod)) {
    if (!isSchema(value)) continue;
    note(exactNames, value.ast, name);
    try {
      const json = Schema.toCodecJson(value).ast;
      note(exactNames, json, name);
      note(encodedNames, SchemaAST.toEncoded(json), name);
    } catch {
      // a schema that cannot derive a JSON codec is reported as an export failure if it is reachable
    }
  }
}
const uniqueName = (m: Map<AST, Set<string>>, ast: AST) => {
  const set = m.get(ast);
  return set && set.size === 1 && !isBareKeyword(ast) ? [...set][0] : undefined;
};

const policyStats = { identifier: 0, exportName: 0, inline: 0 };
const referencePolicy: import("effect/SchemaRepresentation").ReferencePolicy = ({ ast, identifier }) => {
  // Class/TaggedError identifiers arrive as "FooEncoded"; only the encoded side is emitted, so drop the suffix.
  const clean = (id: string) => id.replace(/Encoded$/, "");
  // `identifier` also resolves from a check's annotation (e.g. a refinement named "OrchestrationTurnDiffRange"
  // shared by two structs), so a node's own identifier wins, then a unique export name, then the resolved one.
  const own = ast.annotations?.identifier;
  if (typeof own === "string") {
    policyStats.identifier++;
    return clean(own);
  }
  const byExport = uniqueName(exactNames, ast) ?? uniqueName(encodedNames, ast);
  if (byExport !== undefined) {
    policyStats.exportName++;
    return byExport;
  }
  if (identifier !== undefined) {
    policyStats.identifier++;
    return clean(identifier);
  }
  policyStats.inline++;
  return undefined;
};

// --- 2. Collect roots. Each root gets an index into the shared multi-document. ---
type Root = { schema: AnySchema; codec: "json" | "stringTree" };
const roots: Root[] = [];
const failures: Array<{ where: string; error: string }> = [];
const addRoot = (schema: AnySchema | undefined, codec: Root["codec"] = "json"): number | null => {
  if (schema === undefined) return null;
  roots.push({ schema, codec });
  return roots.length - 1;
};
const unionOf = (schemas: ReadonlyArray<AnySchema>): AnySchema | undefined =>
  schemas.length === 0
    ? undefined
    : schemas.length === 1
      ? schemas[0]
      : Schema.Union(schemas as [AnySchema, ...AnySchema[]]);

// RPC group(s): every RpcGroup exported from contracts.
const rpcGroups = Object.entries(contracts).filter(
  ([, v]) => (typeof v === "object" || typeof v === "function") && v !== null && "requests" in v && v.requests instanceof Map,
) as Array<[string, { requests: Map<string, any> }]>;

type RpcEntry = {
  group: string;
  tag: string;
  stream: boolean;
  payload: number | null;
  success: number | null;
  error: number | null;
  middlewareErrors: string[];
};
const rpcEntries: RpcEntry[] = [];
for (const [groupName, group] of rpcGroups) {
  for (const [tag, rpc] of group.requests) {
    const streamSchemas = RpcSchema.isStreamSchema(rpc.successSchema)
      ? { success: rpc.successSchema.success as AnySchema, error: rpc.successSchema.error as AnySchema }
      : undefined;
    const stream = streamSchemas !== undefined;
    const success = stream ? streamSchemas.success : (rpc.successSchema as AnySchema);
    const errorParts: AnySchema[] = [];
    const baseError = stream ? streamSchemas.error : (rpc.errorSchema as AnySchema);
    if (baseError.ast._tag !== "Never") errorParts.push(baseError);
    const middlewareErrors: string[] = [];
    for (const mw of rpc.middlewares as Set<any>) {
      if (!isSchema(mw.error) || mw.error.ast._tag === "Never") continue;
      middlewareErrors.push(String(mw.key ?? mw.name ?? "middleware"));
      // The middleware error is often already in the RPC's own error union; skip duplicates.
      const has = (p: AnySchema) =>
        p.ast === mw.error.ast || (p.ast._tag === "Union" && p.ast.types.includes(mw.error.ast));
      if (!errorParts.some(has)) errorParts.push(mw.error);
    }
    const payload = rpc.payloadSchema as AnySchema;
    rpcEntries.push({
      group: groupName,
      tag,
      stream,
      payload: payload.ast._tag === "Void" ? null : addRoot(payload),
      success: success.ast._tag === "Void" ? null : addRoot(success),
      error: addRoot(unionOf(errorParts)),
      middlewareErrors,
    });
  }
}

// HTTP APIs: every HttpApi exported from contracts or relay.
const httpApis = [
  ...Object.entries(contracts),
  ...Object.entries(relay).filter(([k]) => !(k in contracts)),
].filter(([, v]) => HttpApi.isHttpApi(v)) as Array<[string, import("effect/http-api/HttpApi").Any]>;

type HttpEntry = {
  api: string;
  group: string;
  name: string;
  method: string;
  path: string;
  params: number | null;
  query: number | null;
  headers: number | null;
  payload: Array<{ contentType: string; encoding: string; schema: number | null }>;
  successes: Array<{ status: number; schema: number | null }>;
  errors: Array<{ status: number; schema: number | null }>;
  middleware: string[];
};
const httpEntries: HttpEntry[] = [];
for (const [apiName, api] of httpApis) {
  HttpApi.reflect(api as any, {
    onGroup: () => {},
    onEndpoint: ({ group, endpoint, successes, errors, middleware }) => {
      const payload: HttpEntry["payload"] = [];
      for (const [contentType, entry] of endpoint.payload) {
        const enc = (entry.encoding as { _tag: string })._tag;
        const schemas = entry.schemas as ReadonlyArray<AnySchema>;
        payload.push({
          contentType,
          encoding: enc,
          schema: enc === "Json" ? addRoot(unionOf(schemas)) : enc === "Stream" ? null : addRoot(unionOf(schemas), "stringTree"),
        });
      }
      const statusList = (m: ReadonlyMap<number, readonly AnySchema[]>) =>
        [...m].map(([status, schemas]) => {
          const nonVoid = schemas.filter((s) => s.ast._tag !== "Void");
          return { status, schema: addRoot(unionOf(nonVoid)) };
        });
      httpEntries.push({
        api: apiName,
        group: group.identifier,
        name: endpoint.identifier,
        method: endpoint.method,
        path: endpoint.path,
        params: addRoot(endpoint.params, "stringTree"),
        query: addRoot(endpoint.query, "stringTree"),
        headers: addRoot(endpoint.headers, "stringTree"),
        payload,
        successes: statusList(successes),
        errors: statusList(errors),
        middleware: [...middleware].map((m: any) => String(m.key ?? m.name)),
      });
    },
  });
}

// Persisted client files that live in contracts.
const persisted = [
  {
    name: "ClientSettings",
    where: "web localStorage key t3code:client-settings:v1; desktop client settings file",
    exportName: "ClientSettingsSchema",
  },
  {
    name: "PersistedSavedEnvironmentRecord",
    where: "desktop saved environments store",
    exportName: "PersistedSavedEnvironmentRecordSchema",
  },
].map((p) => ({ ...p, schema: addRoot(contracts[p.exportName] as AnySchema) }));

// --- 3. Lower every root in one shared reference environment. ---
const codecAst = (r: Root): AST =>
  r.codec === "json" ? Schema.toCodecJson(r.schema).ast : Schema.toCodecJson(Schema.toCodecStringTree(r.schema)).ast;

// Probe each root alone first so one bad schema cannot hide the rest.
const okRoots: number[] = [];
const rootIndex = new Map<number, number>();
for (let i = 0; i < roots.length; i++) {
  try {
    const doc = SchemaRepresentation.toRepresentation(codecAst(roots[i]!), { referencePolicy: () => undefined });
    SchemaRepresentation.toJsonSchemaDocument(doc);
    rootIndex.set(i, okRoots.length);
    okRoots.push(i);
  } catch (e) {
    failures.push({ where: `root#${i}`, error: e instanceof Error ? e.message : String(e) });
  }
}
const multi = SchemaRepresentation.toRepresentations(
  okRoots.map((i) => codecAst(roots[i]!)) as [AST, ...AST[]],
  { referencePolicy },
);
// "error" mode makes records export their value schema even when the key has checks (in "ignore" mode a
// pattern-checked key drops the whole index signature). It also marks structs closed, see $comment.
const jsonDoc = SchemaRepresentation.toJsonSchemaMultiDocument(multi, {
  onExcessProperty: "error",
  includeAnnotationKey: (key) => key === FORWARD_COMPAT_KEY || key === ID_TABLE_KEY,
});

const ref = (i: number | null) => {
  if (i === null) return null;
  const j = rootIndex.get(i);
  return j === undefined ? { $exportFailed: true } : jsonDoc.schemas[j];
};

// --- 4. Metadata the JSON Schema cannot carry. ---

// Scopes. RPC_REQUIRED_SCOPES (server-authoritative) lives in the server, so read it as text instead
// of importing server code; CLIENT_GUARDED_RPC_SCOPES is the client-side gate from contracts.
const scopeValue = (constName: string) => {
  const v = contracts[constName];
  return typeof v === "string" ? v : constName;
};
const parseScopeMap = (file: string) => {
  const src = fs.readFileSync(path.join(repo, file), "utf8");
  const map = new Map<string, string>();
  for (const m of src.matchAll(/\[((?:ORCHESTRATION_V2_)?WS_METHODS)\.(\w+)\]:\s*(\w+)/g)) {
    const tag = (contracts[m[1]!] as Record<string, string>)[m[2]!];
    if (tag) map.set(tag, scopeValue(m[3]!));
  }
  return map;
};
const serverScopes = parseScopeMap("apps/server/src/auth/RpcAuthorization.ts");
const clientGuarded = parseScopeMap("packages/contracts/src/clientRpcPermissions.ts");
for (const e of rpcEntries)
  if (!serverScopes.has(e.tag) && !clientGuarded.has(e.tag)) throw new Error(`no scope declared for ${e.tag}`);

// HTTP callers: HttpApiClient calls look like `client.<group>.<endpoint>(`; raw fetches name the path.
const isTestFile = (f: string) => /(\.test\.|\.spec\.|\.browser\.|__tests__|\/test\/|\/tests\/|fixtures?\/)/.test(f);
const walk = (dir: string): string[] =>
  fs.existsSync(dir)
    ? fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
        const p = path.join(dir, e.name);
        if (e.isDirectory()) return e.name === "node_modules" ? [] : walk(p);
        return /\.(ts|tsx)$/.test(e.name) && !isTestFile(p) ? [p] : [];
      })
    : [];
const clientSources = ["packages/client-runtime/src", "apps/web/src", "apps/mobile/src", "apps/desktop/src"]
  .flatMap((dir) => walk(path.join(repo, dir)))
  .map((f) => fs.readFileSync(f, "utf8"));
const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const clientCalls = (h: HttpEntry) => {
  const stat = h.path.split("/:")[0]!;
  const byPath = stat.length >= 5 ? new RegExp(`["'\`]${escapeRe(stat)}["'\`/?]`) : null;
  const byClient = new RegExp(`(?:\\.${escapeRe(h.group)}|\\b(?:client|urls|api)\\w*)\\.${escapeRe(h.name)}\\(`);
  return clientSources.some((text) => byClient.test(text) || (byPath?.test(text) ?? false));
};

// Source module of every exported schema, so generated Rust mirrors packages/contracts/src.
const exportModule = new Map<string, string>();
const srcDir = path.join(cwd, "src");
const moduleFiles = fs.readdirSync(srcDir).filter((f) => f.endsWith(".ts") && !f.includes(".test.") && f !== "index.ts");
const moduleText = new Map<string, string>();
for (const file of moduleFiles.sort()) {
  const mod = file.replace(/\.ts$/, "");
  moduleText.set(mod, fs.readFileSync(path.join(srcDir, file), "utf8"));
  const exports: Record<string, unknown> = await imp(`src/${file}`);
  for (const [name, value] of Object.entries(exports)) if (isSchema(value) && !exportModule.has(name)) exportModule.set(name, mod);
}
// A definition named by an identifier annotation rather than an export: find the file that spells the name.
const moduleOf = (name: string): string | undefined => {
  const base = name.replace(/_\d+$/, "");
  if (exportModule.has(base)) return exportModule.get(base);
  for (const [mod, text] of moduleText) if (text.includes(`"${base}"`)) return mod;
  return undefined;
};

// --- 5. Emit. ---
const out = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  $comment: [
    "Generated by scripts/contracts/dump.mts (regenerate: scripts/contracts/regen.sh).",
    "Schemas describe the encoded (wire) side of Schema.toCodecJson, the codec RpcSerialization.layerJson and HttpApi JSON bodies use. $ref targets live in definitions.",
    "Objects say additionalProperties: false, but decoders ignore unknown keys (effect's default), so newer servers may add keys.",
    "Optional keys (Schema.optional) may arrive as null: toCodecJson writes undefined as null, and decoding accepts null or a missing key.",
    "x-forward-compatible: the contract decodes an unknown value here to undefined/null instead of failing. Clients should do the same.",
    "x-record-id-table: the response may be id-table compressed when the client opts in (packages/contracts/src/orchestrationV2IdTable.ts).",
    "In-memory patches applied while dumping (scripts/contracts/hooks.mts): brands get identifiers; forward-compatible fields, ModelSelection and the id-table wrapper export their typed shape instead of {}.",
  ].join(" "),
  source: {
    package: "@t3tools/contracts",
    version: JSON.parse(fs.readFileSync(path.join(cwd, "package.json"), "utf8")).version,
    effect: JSON.parse(fs.readFileSync(path.join(cwd, "node_modules/effect/package.json"), "utf8")).version,
  },
  // Frames on the WebSocket (RpcSerialization.layerJson, one JSON message per frame). Hand-copied from
  // effect/dist/rpc/RpcMessage.d.ts (FromClientEncoded / FromServerEncoded) because effect exposes them only as TS types.
  // A stream RPC answers with Chunk frames (values = success elements) then one Exit; unary answers with one Exit.
  // RpcClient sends Ack after consuming each Chunk (RpcClient.js:250) and Ping every 5 s by default (RpcClient.js:918).
  rpcEnvelope: {
    fromClient: {
      anyOf: [
        {
          type: "object",
          properties: {
            _tag: { const: "Request" },
            id: { type: ["string", "number"], description: "client-chosen request id, echoed as requestId" },
            tag: { type: "string", description: "RPC method tag, see rpc[].tag" },
            payload: { description: "rpc[].payload" },
            headers: { type: "array", items: { type: "array", prefixItems: [{ type: "string" }, { type: "string" }] } },
            traceId: { type: "string" },
            spanId: { type: "string" },
            sampled: { type: "boolean" },
          },
          required: ["_tag", "id", "tag", "payload", "headers"],
        },
        { type: "object", properties: { _tag: { const: "Ack" }, requestId: { type: ["string", "number"] } }, required: ["_tag", "requestId"] },
        { type: "object", properties: { _tag: { const: "Interrupt" }, requestId: { type: ["string", "number"] } }, required: ["_tag", "requestId"] },
        { type: "object", properties: { _tag: { const: "Ping" } }, required: ["_tag"] },
        { type: "object", properties: { _tag: { const: "Eof" } }, required: ["_tag"] },
      ],
    },
    fromServer: {
      anyOf: [
        {
          type: "object",
          properties: { _tag: { const: "Chunk" }, requestId: { type: ["string", "number"] }, values: { type: "array", minItems: 1, description: "rpc[].success elements" } },
          required: ["_tag", "requestId", "values"],
        },
        {
          type: "object",
          properties: {
            _tag: { const: "Exit" },
            requestId: { type: ["string", "number"] },
            exit: {
              anyOf: [
                { type: "object", properties: { _tag: { const: "Success" }, value: { description: "rpc[].success (unary) or null (stream)" } }, required: ["_tag"] },
                {
                  type: "object",
                  properties: {
                    _tag: { const: "Failure" },
                    cause: {
                      type: "array",
                      items: {
                        anyOf: [
                          { type: "object", properties: { _tag: { const: "Fail" }, error: { description: "rpc[].error" } }, required: ["_tag", "error"] },
                          { type: "object", properties: { _tag: { const: "Die" }, defect: {} }, required: ["_tag", "defect"] },
                          { type: "object", properties: { _tag: { const: "Interrupt" }, fiberId: { type: ["number", "null"] } }, required: ["_tag"] },
                        ],
                      },
                    },
                  },
                  required: ["_tag", "cause"],
                },
              ],
            },
          },
          required: ["_tag", "requestId", "exit"],
        },
        { type: "object", properties: { _tag: { const: "Defect" }, defect: {} }, required: ["_tag", "defect"] },
        { type: "object", properties: { _tag: { const: "Pong" } }, required: ["_tag"] },
        { type: "object", properties: { _tag: { const: "ClientProtocolError" }, error: {}, requestIds: { type: "array" } }, required: ["_tag", "error"] },
      ],
    },
  },
  rpc: rpcEntries.map((e) => ({
    group: e.group,
    tag: e.tag,
    stream: e.stream,
    payload: ref(e.payload),
    success: ref(e.success),
    error: ref(e.error),
    scope: serverScopes.get(e.tag) ?? clientGuarded.get(e.tag),
    clientGuarded: clientGuarded.has(e.tag),
    ...(e.middlewareErrors.length ? { middlewareErrors: e.middlewareErrors } : {}),
  })),
  http: httpEntries.map((e) => ({
    api: e.api,
    group: e.group,
    name: e.name,
    method: e.method,
    path: e.path,
    params: ref(e.params),
    query: ref(e.query),
    headers: ref(e.headers),
    payload: e.payload.map((p) => ({ ...p, schema: ref(p.schema) })),
    successes: e.successes.map((s) => ({ status: s.status, schema: ref(s.schema) })),
    errors: e.errors.map((s) => ({ status: s.status, schema: ref(s.schema) })),
    middleware: e.middleware,
    clientCalls: clientCalls(e),
  })),
  persisted: persisted.map((p) => ({ name: p.name, where: p.where, exportName: p.exportName, schema: ref(p.schema) })),
  definitions: Object.fromEntries(Object.entries(jsonDoc.definitions).sort(([a], [b]) => a.localeCompare(b))),
  exportFailures: failures.map((f) => {
    const m = /^root#(\d+)$/.exec(f.where);
    if (!m) return f;
    const i = Number(m[1]);
    const owner =
      rpcEntries.find((e) => [e.payload, e.success, e.error].includes(i))?.tag ??
      httpEntries.find((e) =>
        [e.params, e.query, e.headers, ...e.payload.map((p) => p.schema), ...e.successes.map((s) => s.schema), ...e.errors.map((s) => s.schema)].includes(i),
      )?.path ??
      persisted.find((p) => p.schema === i)?.name;
    return { where: owner ?? f.where, error: f.error };
  }),
};

// Structurally equal ASTs stay distinct candidates, so one export can be emitted as X, X_1, X_2.
// Merge a suffixed definition into its base when both compile to the same JSON Schema.
const defs: Record<string, unknown> = out.definitions;
const alias = new Map<string, string>();
const resolveAlias = (n: string): string => (alias.has(n) ? resolveAlias(alias.get(n)!) : n);
const canon = (v: unknown) =>
  JSON.stringify(v).replace(/"#\/\$defs\/([^"]+)"/g, (_, n: string) => `"#/$defs/${resolveAlias(n)}"`);
// Some contract code rebuilds a struct and drops a brand annotation, so X_1 can equal X except that one
// field is {"$ref": "ProviderInstanceId"} in one and {"type": "string"} in the other. Compare with
// scalar refs inlined and keep the copy that names more brands.
const isScalarDef = (v: unknown) =>
  typeof v === "object" && v !== null && !JSON.stringify(v).includes('"$ref"') && !("properties" in v) && !("anyOf" in v);
const canonLoose = (v: unknown) =>
  canon(v).replace(/\{"\$ref":"#\/\$defs\/([^"]+)"\}/g, (whole, n: string) =>
    isScalarDef(defs[n]) ? JSON.stringify(defs[n]) : whole,
  );
const refCount = (v: unknown) => (JSON.stringify(v).match(/"\$ref"/g) ?? []).length;
for (let changed = true; changed; ) {
  changed = false;
  for (const name of Object.keys(defs)) {
    const m = /^(.*)_\d+$/.exec(name);
    const base = m?.[1];
    if (!base || alias.has(name) || !(base in defs)) continue;
    if (canon(defs[name]) === canon(defs[base]) || canonLoose(defs[name]) === canonLoose(defs[base])) {
      if (refCount(defs[name]) > refCount(defs[base])) defs[base] = defs[name];
      alias.set(name, base);
      changed = true;
    }
  }
}
for (const name of alias.keys()) delete defs[name];
const unmergedSuffixed = Object.keys(defs).filter((n) => /_\d+$/.test(n));

// Rewrite "#/$defs/X" refs to "#/definitions/X" so the top-level definitions section resolves.
const resolved: typeof out = JSON.parse(
  JSON.stringify(out).replace(/"#\/\$defs\/([^"]+)"/g, (_, n: string) => `"#/definitions/${resolveAlias(n)}"`),
);

// The reference policy only sees AST identity, so a named schema can still appear inline where the
// contract rebuilt it (NullOr, mapFields, a contextual copy). Replace any inline subschema that is
// byte-identical to a non-trivial definition with a $ref, repeating until nothing changes.
const isNonTrivial = (v: unknown) => {
  if (typeof v !== "object" || v === null) return false;
  const s = JSON.stringify(v);
  return s.length >= 40 && ("properties" in v || "anyOf" in v || "oneOf" in v || "enum" in v);
};
let inlinedRefs = 0;
for (let pass = 0; pass < 10; pass++) {
  const byJson = new Map<string, string>();
  for (const [name, def] of Object.entries(resolved.definitions).sort(([a], [b]) => a.localeCompare(b)))
    if (isNonTrivial(def) && !byJson.has(JSON.stringify(def))) byJson.set(JSON.stringify(def), name);
  let changed = 0;
  const replace = (v: unknown, selfName?: string): unknown => {
    if (typeof v !== "object" || v === null) return v;
    if (Array.isArray(v)) return v.map((x) => replace(x));
    const hit = isNonTrivial(v) ? byJson.get(JSON.stringify(v)) : undefined;
    if (hit !== undefined && hit !== selfName) {
      changed++;
      return { $ref: `#/definitions/${hit}` };
    }
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, replace(x)]));
  };
  for (const [name, def] of Object.entries(resolved.definitions)) {
    // A definition's own root is never replaced by a ref to itself; its children may be.
    const own = def as Record<string, unknown>;
    resolved.definitions[name] = Object.fromEntries(Object.entries(own).map(([k, x]) => [k, replace(x)]));
  }
  resolved.rpc = resolved.rpc.map((r) => replace(r) as (typeof resolved.rpc)[number]);
  resolved.http = resolved.http.map((h) => replace(h) as (typeof resolved.http)[number]);
  resolved.persisted = resolved.persisted.map((p) => replace(p) as (typeof resolved.persisted)[number]);
  inlinedRefs += changed;
  if (changed === 0) break;
}

// Definitions without a module of their own inherit one in the generator (from the type that uses them).
(resolved as Record<string, unknown>).definitionModules = Object.fromEntries(
  Object.keys(resolved.definitions).flatMap((name) => {
    const mod = moduleOf(name);
    return mod === undefined ? [] : [[name, mod]];
  }),
);
const text = JSON.stringify(resolved, null, 2);
fs.writeFileSync(outFile, text + "\n");
console.log(
  JSON.stringify(
    {
      outFile,
      rpcGroups: rpcGroups.map(([n, g]) => `${n}(${g.requests.size})`),
      rpc: rpcEntries.length,
      streams: rpcEntries.filter((e) => e.stream).length,
      httpApis: httpApis.map(([n]) => n),
      http: httpEntries.length,
      roots: roots.length,
      definitions: Object.keys(resolved.definitions).length,
      mergedDuplicates: alias.size,
      inlineCopiesReplacedByRef: inlinedRefs,
      unmergedSuffixed,
      policyStats,
      failures: failures.length,
      bytes: text.length,
    },
    null,
    2,
  ),
);
