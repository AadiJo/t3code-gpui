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
import { assertAllApplied } from "./hooks.mts";

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

fs.writeFileSync(fixturesFile, JSON.stringify(fixtures) + "\n");
console.log(JSON.stringify({ samplesPerSchema: samples, ...stats, fixtureTypes: Object.keys(fixtures).length, fixturesFile }, null, 2));
const byKind = Object.groupBy(problems, (p) => p.kind);
for (const [kind, list] of Object.entries(byKind)) {
  console.log(`\n## ${kind} (${list!.length})`);
  for (const p of list!.slice(0, 40)) console.log(`- ${p.where}: ${p.detail}`);
}
