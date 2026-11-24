// Node-environment vitest for the web-side dumps. T3CODE_REF is the TypeScript checkout;
// T3_HARNESS_DIR holds the test files and a node_modules link to its apps/web/node_modules.
// The web's own Vite config only adds the `~` alias these modules need.
export default {
  root: process.env.T3_HARNESS_DIR,
  cacheDir: `${process.env.T3_HARNESS_DIR}/.vite`,
  resolve: { tsconfigPaths: true, alias: [{ find: /^~\//, replacement: `${process.env.T3CODE_REF}/apps/web/src/` }] },
  server: { fs: { strict: false } },
  test: { include: ["rows.test.ts", "gen.test.ts"], environment: "node" },
};
