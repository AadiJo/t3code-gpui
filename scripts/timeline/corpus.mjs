// Evaluates the corpus literals once and writes the command corpus as plain JSON.
import fs from "node:fs";
const here = new URL(".", import.meta.url).pathname;
const { sources, commands } = JSON.parse(fs.readFileSync(here + "corpus-src.json", "utf8"));
const values = new Set(commands);
for (const src of sources) {
  try {
    const v = new Function("return " + src)();
    if (typeof v === "string" && v.length > 0 && v.length < 4000) values.add(v);
  } catch {}
}
fs.writeFileSync(here + "corpus.json", JSON.stringify([...values]));
console.log(values.size);
