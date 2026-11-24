// Prefix every selector of a generated CSS file with [data-retelens] so our rules cannot leak
// into the host page. `.dark X` becomes `.dark [data-retelens] X`.
import postcss from "postcss";
import fs from "node:fs";
const [, , inp, out] = process.argv;
const css = fs.readFileSync(inp, "utf8");
const root = postcss.parse(css);
root.walkRules((rule) => {
  if (rule.parent && rule.parent.type === "atrule" && /keyframes/.test(rule.parent.name)) return;
  rule.selectors = rule.selectors.map((s) => {
    if (s.startsWith(".dark ")) return ".dark [data-retelens] " + s.slice(6);
    return "[data-retelens] " + s;
  });
});
fs.writeFileSync(out, root.toString());
