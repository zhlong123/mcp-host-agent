import { join } from "node:path";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";

const triple = process.env.TARGET ?? "x86_64-pc-windows-msvc";
const src = join("target", "release", "mcp-host-agent.exe");
const sidecarName = `mcp-host-agent-${triple}.exe`;
const dstDir = join("src-tauri", "bin");
const dst = join(dstDir, sidecarName);

if (!existsSync(src)) {
  console.error(`missing ${src} — run cargo build --release -p mcp-host-agent first`);
  process.exit(1);
}
mkdirSync(dstDir, { recursive: true });
copyFileSync(src, dst);
console.log(`copied ${src} -> ${dst}`);

// tauri build also expects target/release/mcp-host-agent-${triple}.exe
const releaseDst = join("target", "release", sidecarName);
copyFileSync(src, releaseDst);
console.log(`copied ${src} -> ${releaseDst}`);
