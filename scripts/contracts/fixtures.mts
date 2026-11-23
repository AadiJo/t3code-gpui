// Checks rpc-schema.json against real encoded values and saves them as wire fixtures.
// For each RPC payload/success/error schema and each persisted schema, samples random values with
// effect's Arbitrary, encodes them with Schema.toCodecJson (the RPC wire codec), and validates the
// JSON against the dumped JSON Schema with Ajv (draft 2020-12).
//
// Run through regen.sh from <checkout>/packages/contracts:
//   node fixtures.mts <rpc-schema.json> <wire-fixtures.json> [samplesPerSchema]
import * as path from "node:path";
import * as fs from "node:fs";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { assertAllApplied, installPatches } from "./hooks.mts";
installPatches({ idTable: false });

const cwd = process.cwd();
const [specFile, fixturesFile] = process.argv.slice(2);
if (!specFile || !fixturesFile) throw new Error("usage: node fixtures.mts <rpc-schema.json> <wire-fixtures.json> [samples]");
const samples = Number(process.argv[4] ?? 8);
const requireFromCwd = createRequire(path.join(cwd, "package.json"));
const load = <T,>(spec: string): Promise<T> => import(pathToFileURL(requireFromCwd.resolve(spec)).href);
const Schema = await load<typeof import("effect/Schema")>("effect/Schema");
const Arbitrary = await load<typeof import("effect/Arbitrary")>("effect/Arbitrary");
const Effect = await load<typeof import("effect/Effect")>("effect/Effect");
const RpcSchema = await load<typeof import("effect/rpc/RpcSchema")>("effect/rpc/RpcSchema");
const contracts: Record<string, any> = await import(pathToFileURL(path.join(cwd, "src/index.ts")).href);
assertAllApplied();
// ajv is a root devDependency of the checkout (via pnpm), not of contracts.
const rootRequire = createRequire(path.join(cwd, "../../package.json"));
const ajvPath = fs
  .readdirSync(path.join(cwd, "../../node_modules/.pnpm"))
  .filter((d) => d.startsWith("ajv@8"))
  .map((d) => path.join(cwd, "../../node_modules/.pnpm", d, "node_modules/ajv/dist/2020.js"))[0]!;
void rootRequire;
const Ajv2020 = (await import(pathToFileURL(ajvPath).href)).default.default;

type AnySchema = import("effect/Schema").Top;
const spec = JSON.parse(fs.readFileSync(specFile, "utf8"));
const ajv = new Ajv2020({ strict: false, allErrors: false, validateFormats: false });
ajv.addSchema({ $id: "defs", definitions: spec.definitions });
const compile = (root: unknown) =>
  ajv.compile(JSON.parse(JSON.stringify(root).replaceAll('"#/definitions/', '"defs#/definitions/')));

const fixtures: Record<string, unknown[]> = {};
const wellFormed = (v: unknown): unknown =>
  typeof v === "string"
    ? v.toWellFormed()
    : Array.isArray(v)
      ? v.map(wellFormed)
      : typeof v === "object" && v !== null
        ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k.toWellFormed(), wellFormed(x)]))
        : v;
const stats = { checked: 0, valid: 0, invalid: 0, sampleFailed: 0, encodeFailed: 0 };
const problems: Array<{ where: string; kind: string; detail: string }> = [];

async function check(where: string, schema: AnySchema, jsonSchema: unknown) {
  if (jsonSchema === null || jsonSchema === undefined) return;
  const validate = compile(jsonSchema);
  let values: ReadonlyArray<unknown>;
  try {
    // A seed per schema keeps fixtures stable across runs, so a regen diff shows contract changes only.
    values = await Effect.runPromise(Arbitrary.sampleEffect(Arbitrary.schema(schema), { count: samples, seed: where }));
  } catch (e) {
    stats.sampleFailed++;
    problems.push({ where, kind: "sample", detail: (e instanceof Error ? e.message : JSON.stringify(e)).slice(0, 300) });
    return;
  }
  const encode = Schema.encodeUnknownEffect(Schema.toCodecJson(schema));
  for (const value of values) {
    let encoded: unknown;
    try {
      encoded = await Effect.runPromise(encode(value) as any);
    } catch (e) {
      stats.encodeFailed++;
      problems.push({ where, kind: "encode", detail: (e instanceof Error ? e.message : JSON.stringify(e)).slice(0, 300) });
      return;
    }
    stats.checked++;
    // Keep valid samples of named types as fixtures for the Rust round-trip test.
    const refName = (jsonSchema as { $ref?: string }).$ref?.replace("#/definitions/", "");
    // fast-check emits lone UTF-16 surrogates; JSON.stringify escapes them but serde_json rejects them,
    // so fixtures carry well-formed strings only.
    if (refName && validate(encoded))
      (fixtures[refName] ??= []).push(
        wellFormed(encoded),
      );
    if (validate(encoded)) stats.valid++;
    else {
      stats.invalid++;
      problems.push({ where, kind: "invalid", detail: JSON.stringify(validate.errors?.slice(0, 2)).slice(0, 400) });
      return;
    }
  }
}

const group = contracts.WsRpcGroup as { requests: Map<string, any> };
const byTag = new Map<string, any>(spec.rpc.map((r: any) => [r.tag, r]));
for (const [tag, rpc] of group.requests) {
  const entry = byTag.get(tag);
  const stream = RpcSchema.isStreamSchema(rpc.successSchema);
  const success: AnySchema = stream ? rpc.successSchema.success : rpc.successSchema;
  const error: AnySchema = stream ? rpc.successSchema.error : rpc.errorSchema;
  await check(`${tag}.payload`, rpc.payloadSchema, entry.payload);
  await check(`${tag}.success`, success, entry.success);
  if (error.ast._tag !== "Never") await check(`${tag}.error`, error, entry.error);
}
for (const p of spec.persisted) await check(`persisted.${p.name}`, contracts[p.exportName], p.schema);

// Id-table pairs: the same value encoded plain and with `idTable: true`, by the real codec. The Rust
// check expands the compact form and compares it with the plain one.
const idTableFixtures: Record<string, Array<{ plain: unknown; compact: unknown }>> = {};
for (const name of ["OrchestrationV2ThreadBoundedSnapshot", "OrchestrationV2ThreadHistoryPage", "OrchestrationV2ThreadStreamItem"]) {
  const schema: AnySchema = contracts[name];
  const encode = Schema.encodeUnknownEffect(Schema.toCodecJson(schema));
  let values: ReadonlyArray<any> = [];
  try {
    values = await Effect.runPromise(Arbitrary.sampleEffect(Arbitrary.schema(schema), { count: samples * 4, seed: `idTable.${name}` }));
  } catch (e) {
    problems.push({ where: `idTable.${name}`, kind: "sample", detail: (e instanceof Error ? e.message : JSON.stringify(e)).slice(0, 300) });
  }
  for (const value of values) {
    if (typeof value !== "object" || value === null) continue;
    try {
      const { idTable: _idTable, ...rest } = value;
      const plain = wellFormed(await Effect.runPromise(encode(rest) as any));
      const compact = wellFormed(await Effect.runPromise(encode({ ...value, idTable: true }) as any));
      if (typeof compact === "object" && compact !== null && "ids" in compact) (idTableFixtures[name] ??= []).push({ plain, compact });
    } catch (e) {
      problems.push({ where: `idTable.${name}`, kind: "encode", detail: (e instanceof Error ? e.message : String(e)).slice(0, 300) });
    }
  }
}
const idTableFile = path.join(path.dirname(fixturesFile), "id-table-fixtures.json");
fs.writeFileSync(idTableFile, JSON.stringify(idTableFixtures) + "\n");
console.log(JSON.stringify({ idTablePairs: Object.fromEntries(Object.entries(idTableFixtures).map(([k, v]) => [k, v.length])) }));

// Helper oracle: inputs (wire form) and what the TypeScript helper returns for them, so the Rust
// ports are checked against the real implementation.
const helperCases: Record<string, unknown[]> = {};
const sampleEncoded = async (name: string, count: number) => {
  const schema: AnySchema = contracts[name];
  const decoded = await Effect.runPromise(Arbitrary.sampleEffect(Arbitrary.schema(schema), { count, seed: `helper.${name}` }));
  const codec = Schema.toCodecJson(schema);
  const encode = Schema.encodeUnknownEffect(codec);
  const decode = Schema.decodeUnknownEffect(codec);
  // Helpers run on what a client decodes from the wire, not on the raw sample (samples can hold
  // values decoding would normalize, such as untrimmed strings).
  const out: Array<{ value: any; encoded: unknown }> = [];
  for (const sample of decoded) {
    const encoded = wellFormed(await Effect.runPromise(encode(sample) as any));
    out.push({ value: await Effect.runPromise(decode(encoded) as any), encoded });
  }
  return out;
};
const scopes: string[] = contracts.AuthEnvironmentScope.literals;
// Random sessions are mostly unauthenticated, so each sample also yields authenticated variants:
// with its permissions, without them (scopes only, with and without serverUpdateScope), and with
// legacy permissions.
const sessionSchema: AnySchema = contracts.AuthSessionState;
const encodeSession = Schema.encodeUnknownEffect(Schema.toCodecJson(sessionSchema));
const without = <T extends Record<string, unknown>>(o: T, key: string) => {
  const { [key]: _drop, ...rest } = o;
  return rest;
};
for (const { value } of await sampleEncoded("AuthSessionState", 30)) {
  const authed = { ...value, authenticated: true };
  const variants = [
    value,
    authed,
    without(authed, "permissions"),
    { ...without(authed, "permissions"), auth: without(authed.auth, "serverUpdateScope") },
    { ...authed, permissions: ["orchestration:read", "orchestration:operate"] },
  ];
  for (const variant of variants) {
    const session = wellFormed(await Effect.runPromise(encodeSession(variant) as any));
    const v: any = await Effect.runPromise(Schema.decodeUnknownEffect(Schema.toCodecJson(sessionSchema))(session) as any);
    (helperCases.sessionGrantsScope ??= []).push({
      session,
      grants: scopes.filter((scope) => contracts.sessionGrantsScope(v, scope)),
      legacyPermissions: contracts.sessionHasLegacyPermissions(v),
    });
  }
}
for (const { value, encoded } of await sampleEncoded("RepositoryIdentity", 40))
  (helperCases.repositoryGrouping ??= []).push({
    identity: encoded,
    key: contracts.repositoryGroupingKeyOf(value),
    displayName: contracts.repositoryGroupingDisplayNameOf(value) ?? null,
  });
for (const { value, encoded } of await sampleEncoded("OrchestrationV2AppThreadLineage", 40))
  (helperCases.isListedThreadLineage ??= []).push({ lineage: encoded, listed: contracts.isListedThreadLineage(value) });
const max = contracts.TERMINAL_WRITE_MAX_LENGTH as number;
// Inputs as [text, repeat] recipes, outputs as UTF-16 chunk lengths, to keep the file small.
const recipes: Array<Array<[string, number]>> = [
  [],
  [["short", 1]],
  [["a", max]],
  [["a", max - 1], ["\u{1F600}", 1], ["b", 1]],
  [["\u{1F600}", max]],
  [["é", max + 3]],
];
for (const recipe of recipes) {
  const data = recipe.map(([s, n]) => s.repeat(n)).join("");
  const chunkLengths = (contracts.chunkTerminalWrite(data) as string[]).map((c) => c.length);
  (helperCases.chunkTerminalWrite ??= []).push({ recipe, chunkLengths });
}
const helperFile = path.join(path.dirname(fixturesFile), "helper-fixtures.json");
fs.writeFileSync(helperFile, JSON.stringify(helperCases) + "\n");
console.log(JSON.stringify({ helperCases: Object.fromEntries(Object.entries(helperCases).map(([k, v]) => [k, v.length])) }));

fs.writeFileSync(fixturesFile, JSON.stringify(fixtures) + "\n");
console.log(JSON.stringify({ samplesPerSchema: samples, ...stats, fixtureTypes: Object.keys(fixtures).length, fixturesFile }, null, 2));
const byKind = Object.groupBy(problems, (p) => p.kind);
for (const [kind, list] of Object.entries(byKind)) {
  console.log(`\n## ${kind} (${list!.length})`);
  for (const p of list!.slice(0, 40)) console.log(`- ${p.where}: ${p.detail}`);
}
