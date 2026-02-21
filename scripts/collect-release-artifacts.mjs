#!/usr/bin/env node
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';

const platform = process.argv[2];

const validPlatforms = new Set(['linux', 'macos', 'windows', 'all']);
if (!validPlatforms.has(platform)) {
  console.error('Usage: node scripts/collect-release-artifacts.mjs <linux|macos|windows|all>');
  process.exit(1);
}

const repoRoot = process.cwd();
const bundleRoot = resolve(repoRoot, 'src-tauri', 'target');
const outRoot = resolve(repoRoot, 'build');

const platformMatchers = {
  linux: [/\.AppImage$/i, /\.deb$/i, /\.rpm$/i],
  macos: [/\.dmg$/i, /\.app$/i],
  windows: [/\.msi$/i, /\.exe$/i]
};

const bundleDirs = {
  linux: ['appimage', 'deb', 'rpm'],
  macos: ['dmg', 'macos'],
  windows: ['msi', 'nsis']
};

function walk(dir, visitor) {
  for (const entry of readdirSync(dir)) {
    const fullPath = join(dir, entry);
    const st = statSync(fullPath);
    if (st.isDirectory()) {
      walk(fullPath, visitor);
      visitor(fullPath, st);
    } else {
      visitor(fullPath, st);
    }
  }
}

function isInBundlePath(filePath, platformName) {
  const normalized = filePath.replace(/\\/g, '/');
  if (!normalized.includes('/release/bundle/')) {
    return false;
  }

  return bundleDirs[platformName].some((segment) => normalized.includes(`/bundle/${segment}/`));
}

function collectForPlatform(platformName) {
  const destination = resolve(outRoot, platformName);
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });

  const matchers = platformMatchers[platformName];
  const collected = [];

  if (!existsSync(bundleRoot)) {
    return collected;
  }

  walk(bundleRoot, (fullPath, st) => {
    if (!isInBundlePath(fullPath, platformName)) {
      return;
    }

    const fileName = basename(fullPath);
    if (!matchers.some((matcher) => matcher.test(fileName))) {
      return;
    }

    if (st.isDirectory() && !fileName.endsWith('.app')) {
      return;
    }

    const destinationPath = join(destination, fileName);
    cpSync(fullPath, destinationPath, { recursive: st.isDirectory() });
    collected.push(destinationPath);
  });

  return collected;
}

const targets = platform === 'all' ? ['linux', 'macos', 'windows'] : [platform];

let total = 0;
for (const target of targets) {
  const copied = collectForPlatform(target);
  total += copied.length;

  if (copied.length === 0) {
    console.warn(`[collect] No artifacts found for ${target} in src-tauri/target/**/release/bundle`);
  } else {
    console.log(`[collect] ${target}: copied ${copied.length} artifact(s) to build/${target}`);
  }
}

if (total === 0) {
  process.exitCode = 1;
}
