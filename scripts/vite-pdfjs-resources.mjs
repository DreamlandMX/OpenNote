// Serves pdf.js runtime resources (CMaps, standard fonts, ICC profiles, wasm
// decoders) from the pinned pdfjs-dist package at `/pdfjs/`, in dev and in the
// build output. Keeps the PDF reader fully offline without vendoring hundreds of
// files into the repository.
import { cpSync, createReadStream, existsSync, statSync } from 'node:fs';
import { dirname, join, normalize, sep } from 'node:path';
import { createRequire } from 'node:module';

const FOLDERS = ['cmaps', 'standard_fonts', 'iccs', 'wasm'];

/** @returns {import('vite').Plugin} */
export function pdfjsResources() {
  const require = createRequire(import.meta.url);
  const root = dirname(require.resolve('pdfjs-dist/package.json'));
  let outDir = 'dist';
  // Only a real `vite build` publishes resources; the test runner also calls
  // closeBundle with a placeholder outDir, which must not create folders.
  let publish = false;
  return {
    name: 'opennote-pdfjs-resources',
    configResolved(config) {
      outDir = config.build.outDir;
      publish = config.command === 'build' && !process.env.VITEST;
    },
    configureServer(server) {
      server.middlewares.use('/pdfjs/', (req, res, next) => {
        const path = normalize(decodeURIComponent((req.url ?? '').split('?')[0] ?? ''));
        const folder = path.split(/[\\/]/).filter(Boolean)[0];
        const file = join(root, path);
        if (!folder || !FOLDERS.includes(folder) || !file.startsWith(root + sep)
          || !existsSync(file) || !statSync(file).isFile()) {
          next();
          return;
        }
        createReadStream(file).pipe(res);
      });
    },
    closeBundle() {
      if (!publish) return;
      for (const folder of FOLDERS) {
        cpSync(join(root, folder), join(outDir, 'pdfjs', folder), { recursive: true });
      }
    },
  };
}
