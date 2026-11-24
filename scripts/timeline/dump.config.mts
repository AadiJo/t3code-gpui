// Runs the server's recorded provider replay fixtures and writes each final thread
// projection, wire-encoded, to $T3_DUMP_DIR. The reference checkout is not modified:
// a transform plugin adds the dump call in memory.

const dumpPlugin = {
  name: "t3-projection-dump",
  enforce: "pre" as const,
  transform(code: string, id: string) {
    if (!id.endsWith("/testkit/ProviderReplayFixtures.ts")) return;
    const marker = "  input.driver.assertOutput(result, transcript);";
    if (!code.includes(marker)) throw new Error("dump marker missing");
    const header = [
      'import * as __S from "effect/Schema";',
      'import { OrchestrationV2ThreadProjection as __P } from "@t3tools/contracts";',
      'import * as __fs from "node:fs";',
    ].join("\n");
    const dump = `  {
    const __dir = process.env.T3_DUMP_DIR;
    if (__dir) {
      const __encode = __S.encodeSync(__S.toCodecJson(__P));
      const __out = [...result.projections.values()].map((p) => __encode(p));
      __fs.writeFileSync(__dir + "/" + scenario.name.replaceAll("/", "__") + ".json", JSON.stringify(__out));
    }
  }
`;
    return header + "\n" + code.replace(marker, dump + marker);
  },
};

export default async () => {
  const base = (await import(`${process.env.T3CODE_REF}/apps/server/vite.config.ts`)).default as Record<string, any>;
  return {
    ...base,
    root: `${process.env.T3CODE_REF}/apps/server`,
    cacheDir: `${process.env.T3_DUMP_DIR}/../.vite-server`,
    plugins: [dumpPlugin, ...(base.plugins ?? [])],
  };
};
