// For each case in a fixture, prints the verdict of chdsbd's fork of
// @snyk/github-codeowners, which the VS Code extension uses: 1 if the line
// owns the file, 0 if not, E if the line is rejected.
const fs = require("fs");
const os = require("os");
const path = require("path");
const {
  OwnershipEngine,
} = require("@snyk/github-codeowners/dist/lib/ownership/OwnershipEngine.js");

// The library only reads CODEOWNERS from a file.
const codeowners = path.join(
  fs.mkdtempSync(path.join(os.tmpdir(), "compare-")),
  "CODEOWNERS",
);
const cases = fs
  .readFileSync(process.argv[2], "utf8")
  .split("\n")
  .filter((l) => l && !l.startsWith("#"));
for (const c of cases) {
  const [line, file] = c.split(" | ");
  fs.writeFileSync(codeowners, line + "\n");
  try {
    const engine = OwnershipEngine.FromCodeownersFile(codeowners);
    if (engine.errors.length || engine.getRules().length === 0) {
      console.log("E");
    } else {
      console.log(engine.calcFileOwnership(file) ? "1" : "0");
    }
  } catch {
    console.log("E");
  }
}
fs.rmSync(path.dirname(codeowners), { recursive: true });
