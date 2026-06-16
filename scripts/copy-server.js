import { copyFileSync, mkdirSync, existsSync } from "fs";
import { join } from "path";

const triple = process.env.CARGO_BUILD_TARGET || "x86_64-pc-windows-msvc";
const src = join("target", "release", "perspective-agent.exe");
const dir = join("src-tauri", "bin");
const dest = join(dir, `perspective-agent-${triple}.exe`);

if (!existsSync(src)) {
  console.error(`missing ${src} — run cargo build --release -p perspective-agent first`);
  process.exit(1);
}

mkdirSync(dir, { recursive: true });
copyFileSync(src, dest);
console.log(`copied ${src} -> ${dest}`);
