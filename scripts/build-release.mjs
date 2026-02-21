#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

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
const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..');
const zigRunnerPath = resolve(repoRoot, 'scripts', 'tauri-zigbuild-runner.sh');
const toolShimDir = resolve(repoRoot, '.codex-tool-shims');

if (!hostTarget) {
  console.error(`[build] Unsupported host platform: ${host}`);
  process.exit(1);
}

const preferredTargetTriples = {
  linux: ['x86_64-unknown-linux-gnu', 'x86_64-unknown-linux-musl', 'aarch64-unknown-linux-gnu', 'aarch64-unknown-linux-musl'],
  macos:
    process.arch === 'arm64'
      ? ['aarch64-apple-darwin', 'x86_64-apple-darwin']
      : ['x86_64-apple-darwin', 'aarch64-apple-darwin'],
  windows:
    host === 'win32'
      ? process.arch === 'arm64'
        ? ['aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu']
        : ['x86_64-pc-windows-msvc', 'x86_64-pc-windows-gnu', 'aarch64-pc-windows-msvc']
      : ['x86_64-pc-windows-gnu', 'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc']
};

function run(cmd, args, { env } = {}) {
  const result = spawnSync(cmd, args, {
    stdio: 'inherit',
    env: env ? { ...process.env, ...env } : process.env
  });
  return result.status === 0;
}

function runQuiet(cmd, args) {
  const result = spawnSync(cmd, args, { stdio: 'pipe', encoding: 'utf8' });
  return { ok: result.status === 0, stdout: result.stdout ?? '' };
}

function commandExists(cmd) {
  return runQuiet('sh', ['-lc', `command -v ${cmd}`]).ok;
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

function platformForTriple(triple) {
  if (triple.includes('apple-darwin')) return 'macos';
  if (triple.includes('windows')) return 'windows';
  if (triple.includes('linux')) return 'linux';
  return 'unknown';
}

function ensureZigRunnerDeps() {
  if (!commandExists('cargo-zigbuild')) {
    console.log('[build] Installing cargo-zigbuild...');
    const installed = run('cargo', ['install', 'cargo-zigbuild', '--locked']);
    if (!installed) {
      console.error('[build] Failed to install cargo-zigbuild.');
      process.exit(1);
    }
  }

  if (!commandExists('zig')) {
    console.error('[build] zig is required for cross-target Linux/Windows builds via cargo-zigbuild.');
    console.error('[build] Install zig (for example: brew install zig) and rerun.');
    process.exit(1);
  }
}

function ensureWindowsGnuTools() {
  const required = ['x86_64-w64-mingw32-dlltool', 'x86_64-w64-mingw32-windres'];
  const missing = required.filter((tool) => !commandExists(tool));
  if (missing.length === 0) {
    return true;
  }

  const autoInstall = process.env.BUILD_AUTO_INSTALL_TOOLCHAINS === '1';
  if (autoInstall && commandExists('brew')) {
    console.log('[build] Installing mingw-w64 via Homebrew for Windows GNU cross-build tooling...');
    const installed = run('brew', ['install', 'mingw-w64']);
    if (installed) {
      return required.every((tool) => commandExists(tool));
    }
  }

  console.error(`[build] Missing Windows GNU tools: ${missing.join(', ')}`);
  console.error('[build] Install with: brew install mingw-w64');
  console.error('[build] Or rerun with BUILD_AUTO_INSTALL_TOOLCHAINS=1 to auto-install.');
  return false;
}

function ensureWindowsNsisEnv() {
  if (commandExists('makensis.exe')) {
    return {};
  }

  const autoInstall = process.env.BUILD_AUTO_INSTALL_TOOLCHAINS === '1';
  if (!commandExists('makensis') && autoInstall && commandExists('brew')) {
    console.log('[build] Installing nsis via Homebrew for Windows installer bundling...');
    const installed = run('brew', ['install', 'nsis']);
    if (!installed) {
      console.error('[build] Failed to install nsis.');
      return null;
    }
  }

  if (!commandExists('makensis') && !commandExists('makensis.exe')) {
    console.error('[build] NSIS is required to bundle Windows installers.');
    console.error('[build] Install with: brew install nsis');
    return null;
  }

  if (commandExists('makensis.exe')) {
    return {};
  }

  mkdirSync(toolShimDir, { recursive: true });
  const shimPath = resolve(toolShimDir, 'makensis.exe');
  writeFileSync(shimPath, '#!/usr/bin/env bash\nexec makensis "$@"\n');
  chmodSync(shimPath, 0o755);
  return { PATH: `${toolShimDir}:${process.env.PATH}` };
}

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
    const triplePlatform = platformForTriple(triple);
    const useZigRunner = triplePlatform !== 'unknown' && triplePlatform !== hostTarget;
    const buildArgs = ['tauri', 'build', '--target', triple];
    let envOverrides = {};
    if (useZigRunner) {
      ensureZigRunnerDeps();
      buildArgs.push('--runner', zigRunnerPath);
    }
    if (triple === 'x86_64-pc-windows-gnu' && host !== 'win32') {
      const hasGnuTools = ensureWindowsGnuTools();
      if (!hasGnuTools) {
        if (continueOnFailure) {
          console.error(`[build] ${target} build failed for ${triple}.`);
          return false;
        }
        process.exit(1);
      }

      const nsisEnv = ensureWindowsNsisEnv();
      if (!nsisEnv) {
        if (continueOnFailure) {
          console.error(`[build] ${target} build failed for ${triple}.`);
          return false;
        }
        process.exit(1);
      }
      envOverrides = { ...envOverrides, ...nsisEnv };
    }

    const ok = run('bunx', buildArgs, { env: envOverrides });
    if (!ok) {
      if (target === 'windows' && host !== 'win32') {
        console.error('[build] Hint: set TAURI_TARGET_WINDOWS=x86_64-pc-windows-gnu if MSVC toolchain is missing.');
      }
      if (target === 'linux' && host !== 'linux') {
        console.error('[build] Hint: Linux cross-builds for Tauri need pkg-config/sysroot for GTK/WebKit (or a dedicated Linux build environment).');
      }
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
