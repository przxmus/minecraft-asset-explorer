#!/usr/bin/env node
import { spawnSync } from 'node:child_process';

const mode = process.argv[2];
const validModes = new Set(['linux', 'macos', 'windows', 'all']);

if (!validModes.has(mode)) {
  console.error('Usage: node scripts/build-release.mjs <linux|macos|windows|all>');
  process.exit(1);
}

const hostToTarget = {
  linux: 'linux',
  darwin: 'macos',
  win32: 'windows'
};

const host = process.platform;
const hostTarget = hostToTarget[host];

if (!hostTarget) {
  console.error(`[build] Unsupported host platform: ${host}`);
  process.exit(1);
}

function run(cmd, args) {
  const result = spawnSync(cmd, args, { stdio: 'inherit' });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function buildTarget(target) {
  if (target !== hostTarget) {
    console.error(`[build] Cannot build ${target} bundles on ${hostTarget}. Run this command on ${target}.`);
    process.exit(1);
  }

  console.log(`[build] Building ${target} release bundles...`);
  run('bunx', ['tauri', 'build']);

  console.log(`[build] Collecting ${target} artifacts...`);
  run('node', ['scripts/collect-release-artifacts.mjs', target]);
}

if (mode === 'all') {
  console.log(`[build] build:all on ${hostTarget} builds local ${hostTarget} artifacts only.`);
  console.log('[build] For full cross-platform release, run build on each OS (Linux, macOS, Windows) and merge ./build folders.');
  buildTarget(hostTarget);
  process.exit(0);
}

buildTarget(mode);
