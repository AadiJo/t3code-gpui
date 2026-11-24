// Generates Rust tables from the fork's t3 tool presentation and sentences.
import * as fs from "node:fs";
import { test } from "vite-plus/test";

const R = `${process.env.T3CODE_REF}/packages`;
const OUT = process.env.T3_GEN_OUT ?? "";

const rustString = (s: string) => JSON.stringify(s);
const variant = (s: string) => s.split("-").map((p) => p[0]!.toUpperCase() + p.slice(1)).join("");

test("generate t3 tool tables", async () => {
  const shared = await import(`${R}/shared/src/t3McpToolPresentation.ts`);
  const sentences = await import(`${R}/client-runtime/src/work-log/t3ToolSentences.ts`);
  fs.mkdirSync(OUT, { recursive: true });
  const actions = fs.readFileSync(process.env.T3_SUMMARY_ACTIONS ?? "", "utf8").trim().split("\n");
  const names = [...shared.T3_MCP_TOOL_NAMES].sort();
  let defs = "";
  let sentenceArms = "";
  let changeArms = "";
  for (const name of names) {
    const d = shared.resolveT3McpToolDefinition(name);
    const icon = { "t3-code": "T3Code", browser: "Browser", device: "Device", "pull-request": "PullRequest" }[d.icon as string];
    defs += `    T3ToolDefinition { name: ${rustString(name)}, display_name: ${rustString(d.displayName)}, labels: [${d.labels.map(rustString).join(", ")}], icon: T3ToolIcon::${icon}, summary_action: T3SummaryAction::${variant(d.summaryAction)} },\n`;
    const s = sentences.t3ToolSentence(d.labels);
    if (s) {
      sentenceArms += `        ${rustString(name)} => t!(${rustString(s.detail)}, status = status),\n`;
      if (s.changeRequest) changeArms += `        ${rustString(name)} => t!(${rustString(s.changeRequest)}, status = status, changeRequest = change_request, number = number),\n`;
    }
  }
  const enumBody = actions.map((a) => `    ${variant(a)},`).join("\n");
  const asStr = actions.map((a) => `            Self::${variant(a)} => ${rustString(a)},`).join("\n");
  fs.writeFileSync(`${OUT}/t3_tools_table.rs`, `${enumBody}\n----\n${asStr}\n----\n${defs}`);
  fs.writeFileSync(`${OUT}/t3_sentences.rs`, `${sentenceArms}----\n${changeArms}`);
});
