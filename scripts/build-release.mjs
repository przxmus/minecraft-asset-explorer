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

const defaultTargetTriples = {
  linux: ['x86_64-unknown-linux-gnu'],
  macos: [process.arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin'],
  windows: ['x86_64-pc-windows-msvc']
};

function run(cmd, args) {
  const result = spawnSync(cmd, args, { stdio: 'inherit' });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function resolveTargetTriples(target) {
  const envKey = `TAURI_TARGET_${target.toUpperCase()}`;
  const raw = process.env[envKey];
  if (!raw) {
    return defaultTargetTriples[target];
  }

  const triples = raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);

  if (triples.length === 0) {
    console.error(`[build] ${envKey} is set but empty. Provide one or more target triples.`);
    process.exit(1);
  }

  return triples;
}

function buildTarget(target) {
  const triples = resolveTargetTriples(target);

  for (const triple of triples) {
    console.log(`[build] Building ${target} release bundles for target ${triple}...`);
    run('bunx', ['tauri', 'build', '--target', triple]);
  }

  console.log(`[build] Collecting ${target} artifacts...`);
  run('node', ['scripts/collect-release-artifacts.mjs', target]);
}

if (mode === 'all') {
  const targets = ['macos', 'linux', 'windows'];
  console.log(`[build] build:all on ${hostTarget} will attempt: ${targets.join(', ')}.`);
  console.log('[build] Cross-target builds require matching Rust targets and linker/toolchain support.');
  for (const target of targets) {
    buildTarget(target);
  }
  process.exit(0);
}

buildTarget(mode);
