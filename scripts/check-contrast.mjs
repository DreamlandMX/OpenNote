import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const tokensCss = readFileSync(join(root, 'src/modules/platform-shell/ui/tokens.css'), 'utf8');
const shellCss = readFileSync(join(root, 'src/modules/platform-shell/ui/shell.css'), 'utf8');

function tokenBlock(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(tokensCss);
  if (!match) throw new Error(`missing token block: ${selector}`);
  const values = {};
  for (const declaration of match[1].split(';')) {
    const separator = declaration.indexOf(':');
    if (separator < 0) continue;
    const name = declaration.slice(0, separator).trim();
    if (name.startsWith('--')) values[name.slice(2)] = declaration.slice(separator + 1).trim();
  }
  return values;
}

function rgb(hex) {
  if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`not a six-digit hex color: ${hex}`);
  return {
    r: Number.parseInt(hex.slice(1, 3), 16),
    g: Number.parseInt(hex.slice(3, 5), 16),
    b: Number.parseInt(hex.slice(5, 7), 16),
  };
}

function luminance(color) {
  const channels = [color.r, color.g, color.b].map((channel) => {
    const value = channel / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
}

function contrast(foreground, background) {
  const [high, low] = [luminance(rgb(foreground)), luminance(rgb(background))].sort(
    (left, right) => right - left,
  );
  return (high + 0.05) / (low + 0.05);
}

function composite(foreground, background, alpha) {
  const front = rgb(foreground);
  const back = rgb(background);
  const channels = [front.r, front.g, front.b].map((channel, index) =>
    Math.round(channel * alpha + [back.r, back.g, back.b][index] * (1 - alpha)),
  );
  return `#${channels.map((channel) => channel.toString(16).padStart(2, '0')).join('')}`;
}

const light = tokenBlock(':root');
const dark = tokenBlock(":root[data-theme='dark']");
const systemDark = tokenBlock(":root[data-theme='system']");
const failures = [];

function check(label, foreground, background, minimum) {
  if (!foreground || !background) {
    failures.push(label);
    console.log(`FAIL ${label}: missing foreground or background token`);
    return;
  }
  const ratio = contrast(foreground, background);
  const verdict = ratio >= minimum ? 'PASS' : 'FAIL';
  console.log(`${verdict} ${label}: ${ratio.toFixed(2)}:1 (minimum ${minimum}:1)`);
  if (verdict === 'FAIL') failures.push(label);
}

const required = [
  'background',
  'navigation',
  'surface',
  'text',
  'secondary',
  'danger',
  'danger-contrast',
  'focus',
  'highlight',
];
for (const [name, colors] of [
  ['light', light],
  ['dark', dark],
]) {
  for (const token of required) {
    if (!/^#[0-9a-f]{6}$/i.test(colors[token] ?? '')) {
      failures.push(`${name} ${token}`);
      console.log(`FAIL ${name} --${token}: missing or not a six-digit hex color`);
    }
  }
}

for (const [name, colors] of [
  ['light', light],
  ['dark', dark],
]) {
  for (const surface of ['background', 'surface', 'navigation']) {
    check(`${name} text on ${surface}`, colors.text, colors[surface], 4.5);
    check(`${name} secondary on ${surface}`, colors.secondary, colors[surface], 4.5);
    check(`${name} danger on ${surface}`, colors.danger, colors[surface], 4.5);
    check(`${name} focus on ${surface}`, colors.focus, colors[surface], 3);
    check(
      `${name} 55% disabled text on ${surface}`,
      composite(colors.text, colors[surface], 0.55),
      colors[surface],
      3,
    );
  }
  check(`${name} text on highlight`, colors.text, colors.highlight, 4.5);
  check(`${name} filled danger foreground`, colors['danger-contrast'], colors.danger, 4.5);
}

const samePalette =
  Object.keys(dark).length === Object.keys(systemDark).length &&
  Object.entries(dark).every(([token, value]) => systemDark[token] === value);
if (!samePalette) {
  failures.push('system dark palette');
  console.log('FAIL system dark palette differs from the explicit dark palette');
} else {
  console.log('PASS system dark palette matches the explicit dark palette');
}

const dangerRule = /button\.danger\s*\{([^}]*)\}/.exec(shellCss);
if (!dangerRule || !dangerRule[1].includes('color: var(--danger-contrast)')) {
  failures.push('button.danger foreground');
  console.log('FAIL button.danger does not use var(--danger-contrast)');
} else if (!dangerRule[1].includes('background: var(--danger)')) {
  failures.push('button.danger background');
  console.log('FAIL button.danger does not use var(--danger)');
} else {
  console.log('PASS button.danger uses the dedicated danger foreground and background tokens');
}

if (failures.length > 0) {
  console.error(`\n${failures.length} contrast check(s) failed: ${failures.join(', ')}`);
  process.exitCode = 1;
} else {
  console.log('\nAll theme contrast checks passed.');
}
