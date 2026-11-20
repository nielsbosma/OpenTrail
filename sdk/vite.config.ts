import { defineConfig } from 'vite';

// Two builds of one entry: `sdk.js`, a script tag that sets `window.OpenTrail` and starts itself
// from its data attributes, and `index.js` for bundlers.
export default defineConfig({
  build: {
    target: 'es2020',
    lib: {
      entry: 'src/index.ts',
      name: 'OpenTrail',
      formats: ['iife', 'es'],
      fileName: (format) => (format === 'iife' ? 'sdk.js' : 'index.js'),
    },
  },
});
