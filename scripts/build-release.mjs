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

const preferredTargetTriples = {
  linux:
    process.arch === 'arm64'
      ? ['aarch64-unknown-linux-gnu', 'aarch64-unknown-linux-musl', 'x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl']
      : ['x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-gnu', 'aarch64-unknown-linux-musl'],
  macos:
    process.arch === 'arm64'
      ? ['aarch64-apple-darwin', 'x86_64-apple-darwin']
      : ['x86_64-apple-darwin', 'aarch64-apple-darwin'],
  windows:
    process.arch === 'arm64'
      ? ['aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu']
      : ['x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu', 'aarch64-pc-windows-msvc']
};

function run(cmd, args) {
  const result = spawnSync(cmd, args, { stdio: 'inherit' });
  return result.status === 0;
}

function listInstalledRustTargets() {
  const result = spawnSync('rustup', ['target', 'list', '--installed'], { encoding: 'utf8' });
  if (result.status !== 0) {
    console.warn('[build] Could not read installed Rust targets; using fallback defaults.');
    return new Set();
  }

  return new Set(
    result.stdout
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
  );
}

const installedRustTargets = listInstalledRustTargets();

function resolveDefaultTargetTriple(target) {
  const preferred = preferredTargetTriples[target];
  const installed = preferred.find((triple) => installedRustTargets.has(triple));

  if (installed) {
    return installed;
  }

  const fallback = preferred[0];
  console.warn(
    `[build] No preferred ${target} target is currently installed (${preferred.join(', ')}). Falling back to ${fallback}.`
  );
  return fallback;
}

function resolveTargetTriples(target) {
  const envKey = `TAURI_TARGET_${target.toUpperCase()}`;
  const raw = process.env[envKey];
  if (!raw) {
    return [resolveDefaultTargetTriple(target)];
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

function buildTarget(target, { continueOnFailure = false } = {}) {
  const triples = resolveTargetTriples(target);

  for (const triple of triples) {
    console.log(`[build] Building ${target} release bundles for target ${triple}...`);
    const ok = run('bunx', ['tauri', 'build', '--target', triple]);
    if (!ok) {
      if (continueOnFailure) {
        console.error(`[build] ${target} build failed for ${triple}.`);
        return false;
      }
      process.exit(1);
    }
  }

  console.log(`[build] Collecting ${target} artifacts...`);
  const collected = run('node', ['scripts/collect-release-artifacts.mjs', target]);
  if (!collected) {
    if (continueOnFailure) {
      console.error(`[build] Artifact collection failed for ${target}.`);
      return false;
    }
    process.exit(1);
  }

  return true;
}

if (mode === 'all') {
  const targets = ['macos', 'linux', 'windows'];
  console.log(`[build] build:all on ${hostTarget} will attempt: ${targets.join(', ')}.`);
  console.log('[build] Cross-target builds require matching Rust targets and linker/toolchain support.');
  const failedTargets = [];
  for (const target of targets) {
    const ok = buildTarget(target, { continueOnFailure: true });
    if (!ok) {
      failedTargets.push(target);
    }
  }
  if (failedTargets.length > 0) {
    console.error(`[build] build:all completed with failures: ${failedTargets.join(', ')}`);
    process.exit(1);
  }
  process.exit(0);
}

buildTarget(mode, { continueOnFailure: false });
