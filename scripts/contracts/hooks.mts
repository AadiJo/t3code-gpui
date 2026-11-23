// In-memory source patches applied while dump.mts and fixtures.mts load the contracts.
// Nothing on disk changes. Each patch asserts its target text so a contract change fails loudly.
//
// Why: the JSON Schema we want for codegen is "what the server sends and accepts", but a few
// contract helpers deliberately decode from Schema.Unknown (forward compatibility), which makes
// the strict encoded side `{}` and erases the real type. The patches swap the Unknown source for
// the value's own encoded schema and tag the node so generators can keep the leniency.
import { registerHooks } from "node:module";

export const FORWARD_COMPAT_KEY = "x-forward-compatible";
export const ID_TABLE_KEY = "x-record-id-table";

type Patch = { file: RegExp; find: string; replace: string; idTable?: true };

const patches: Patch[] = [
  {
    // Schema.brand returns a schema with the SAME AST as its base, so ThreadId, ProjectId and
    // TrimmedNonEmptyString are indistinguishable at runtime. Give each brand an identifier.
    file: /\/effect\/dist\/Schema\.js$/,
    find: "return schema => make(schema.ast, {\n    schema,\n    identifier\n  });",
    replace: "return schema => make(schema.annotate({ identifier }).ast, {\n    schema,\n    identifier\n  });",
  },
  {
    // ForwardCompatibleOptional: Unknown -> UndefinedOr(value)
    file: /\/packages\/contracts\/src\/baseSchemas\.ts$/,
    find: "  return Schema.optionalKey(\n    Schema.Unknown.pipe(",
    replace: `  return Schema.optionalKey(\n    Schema.toEncoded(value).annotate({ "${FORWARD_COMPAT_KEY}": true }).pipe(`,
  },
  {
    // ForwardCompatibleNullable: Unknown -> NullOr(value)
    file: /\/packages\/contracts\/src\/baseSchemas\.ts$/,
    find: "  return Schema.Unknown.pipe(\n    Schema.decodeTo(\n      Schema.NullOr(value),",
    replace: `  return Schema.NullOr(Schema.toEncoded(value)).annotate({ "${FORWARD_COMPAT_KEY}": true }).pipe(\n    Schema.decodeTo(\n      Schema.NullOr(value),`,
  },
  {
    // OmittedWhenNull: optionalKey(Unknown) -> optionalKey(value); a value that does not decode becomes null.
    file: /\/packages\/contracts\/src\/baseSchemas\.ts$/,
    find: "  return Schema.optionalKey(Schema.Unknown).pipe(\n    Schema.decodeTo(\n      Schema.NullOr(value),",
    replace: `  return Schema.optionalKey(Schema.toEncoded(value).annotate({ "${FORWARD_COMPAT_KEY}": true })).pipe(\n    Schema.decodeTo(\n      Schema.NullOr(value),`,
  },
  {
    // hiddenThreadTags decodes Array(Unknown) and drops non-strings; the server only ever encodes strings.
    file: /\/packages\/contracts\/src\/settings\.ts$/,
    find: "  hiddenThreadTags: Schema.Array(Schema.Unknown).pipe(",
    replace: "  hiddenThreadTags: Schema.Array(Schema.String).pipe(",
  },
  {
    // ModelSelection decodes a legacy {provider?, instanceId?, model, options?} bag of unknowns and
    // always encodes {instanceId, model, options?}. Emit the shape the server writes.
    file: /\/packages\/contracts\/src\/modelSelection\.ts$/,
    find: "export const ModelSelection = ModelSelectionSource.pipe(",
    replace: `export const ModelSelection = Schema.toEncoded(ModelSelectionWire).annotate({ identifier: "ModelSelection", "${FORWARD_COMPAT_KEY}": true }).pipe(`,
  },
  {
    // withRecordIdTable decodes from an opaque ParsedJson declaration, so the snapshot, history page and
    // thread stream item export as `{}`. Without the opt-in (idTable: true) the wire equals the inner
    // codec; with it, id-like keys hold indexes into a top-level `ids` array (orchestrationV2IdTable.ts).
    idTable: true,
    file: /\/packages\/contracts\/src\/orchestrationV2IdTable\.ts$/,
    // Early-return the inner schema itself (tagged) so its children keep their names; the rest is dead code.
    find: "  const codec = Schema.toCodecJson(schema);\n",
    replace: `  return schema.annotate({ "${ID_TABLE_KEY}": true });\n  const codec = Schema.toCodecJson(schema);\n`,
  },
];

export const applied = new Map<string, number>();
let active: Patch[] = [];

/**
 * Registers the patches; call before the first dynamic import of effect or the contracts.
 * `idTable: false` keeps the real id-table codec (fixtures.mts encodes id-table samples with it).
 */
export const installPatches = ({ idTable }: { idTable: boolean }) => {
  active = patches.filter((p) => idTable || !p.idTable);
  registerHooks({
    load(url, context, nextLoad) {
      const result = nextLoad(url, context);
      const mine = active.filter((p) => p.file.test(url));
      if (mine.length === 0) return result;
      let src = String(result.source);
      for (const p of mine) {
        if (!src.includes(p.find)) throw new Error(`patch target missing in ${url}:\n${p.find}`);
        src = src.replace(p.find, p.replace);
        applied.set(p.find.slice(0, 50), (applied.get(p.find.slice(0, 50)) ?? 0) + 1);
      }
      return { ...result, source: src };
    },
  });
};

export const assertAllApplied = () => {
  if (applied.size !== active.length)
    throw new Error(`only ${applied.size}/${active.length} source patches applied: ${[...applied.keys()].join(" | ")}`);
};
