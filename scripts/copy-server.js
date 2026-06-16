import { copyFileSync, mkdirSync, existsSync } from "fs";
import { join } from "path";

const triple = process.env.CARGO_BUILD_TARGET || "x86_64-pc-windows-msvc";
const src = join("target", "release", "perspective-agent.exe");
const sidecarName = `perspective-agent-${triple}.exe`;

const targets = [
  join("src-tauri", "bin", sidecarName),
  join("target", "release", sidecarName),
];

if (!existsSync(src)) {
  console.error(`missing ${src} — run cargo build --release -p perspective-agent first`);
  process.exit(1);
}

for (const dest of targets) {
  mkdirSync(join(dest, ".."), { recursive: true });
  copyFileSync(src, dest);
  console.log(`copied ${src} -> ${dest}`);
}
