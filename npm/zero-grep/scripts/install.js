const { mkdirSync, createWriteStream } = require("node:fs");
const { join } = require("node:path");
const https = require("node:https");
const { pipeline } = require("node:stream/promises");

const target = {
  "darwin-arm64": "aarch64-apple-darwin",
  "darwin-x64": "x86_64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc.exe",
}[`${process.platform}-${process.arch}`];

if (!target) {
  console.error(`No prebuilt zero-grep binary for ${process.platform}-${process.arch}. Install with cargo instead.`);
  process.exit(1);
}

const pkg = require("../package.json");
const asset = `zg-${target}`;
const url = `https://github.com/ml-lubich/zero-grep/releases/download/v${pkg.version}/${asset}`;
const destination = join(__dirname, "..", "native");
const output = join(destination, process.platform === "win32" ? "zg.exe" : "zg");
mkdirSync(destination, { recursive: true });

function download(address, redirects = 5) {
  return new Promise((resolve, reject) => {
    https.get(address, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        if (!redirects) return reject(new Error("too many release download redirects"));
        return resolve(download(response.headers.location, redirects - 1));
      }
      if (response.statusCode !== 200) return reject(new Error(`binary download returned HTTP ${response.statusCode}`));
      pipeline(response, createWriteStream(output)).then(resolve, reject);
    }).on("error", reject);
  });
}

download(url)
  .then(async () => {
    if (process.platform !== "win32") {
      const { chmodSync } = require("node:fs");
      chmodSync(output, 0o755);
    }
  })
  .catch((error) => {
    console.error(`Could not install zero-grep: ${error.message}`);
    process.exit(1);
  });
