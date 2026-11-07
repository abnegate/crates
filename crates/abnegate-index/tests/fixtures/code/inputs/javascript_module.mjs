import { readFile } from 'node:fs/promises';
import path from 'node:path';

export const CONFIG_NAME = 'widgets.config.json';

export async function loadConfig(directory) {
  const file = path.join(directory, CONFIG_NAME);
  const raw = await readFile(file, 'utf8');
  const parsed = JSON.parse(raw);
  return { ...defaults(), ...parsed };
}

function defaults() {
  return { retries: 3, verbose: false, outputDirectory: 'dist' };
}

export function mergeConfigs(base, override) {
  const merged = { ...base };
  for (const [key, value] of Object.entries(override)) {
    if (value !== undefined) {
      merged[key] = value;
    }
  }
  return merged;
}
