/**
 * ESLint flat config for FerriScribe (Svelte 5 runes + TypeScript + Vite).
 *
 * Focuses on catching real bugs (unused vars, no-undef, no-console in prod
 * paths) rather than style bikeshedding. Svelte-specific rules via
 * eslint-plugin-svelte. Prettier handles formatting separately.
 */
import js from '@eslint/js';
import ts from 'typescript-eslint';
import svelte from 'eslint-plugin-svelte';
import prettier from 'eslint-config-prettier';
import globals from 'globals';

export default [
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs['flat/recommended'],
  prettier,
  {
    languageOptions: {
      globals: {
        ...globals.browser,
        ...globals.node,
        // Injected by Vite define in vite.config.ts
        __APP_VERSION__: 'readonly',
      },
    },
  },
  {
    rules: {
      // Allow console.error/warn (legitimate error logging) but flag console.log
      // (debug leftovers). The app's intentional log facade is in api/logging.ts.
      'no-console': ['warn', { allow: ['warn', 'error'] }],
      // Prefer const for never-reassigned bindings (catches accidental mutation).
      'prefer-const': 'error',
      // No unused vars (TS already covers this, but ESLint catches it earlier
      // and with better messages). Ignore args prefixed with _.
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      // `any` is debt in test mocks and a few boundary types; warn rather than
      // error so CI doesn't block on the existing 27 sites. New code should
      // avoid it; this makes it visible without breaking the build.
      '@typescript-eslint/no-explicit-any': 'warn',
      // Svelte each-key and reactivity preferences are best-practice; warn to
      // surface them without blocking the build on the existing backlog.
      'svelte/require-each-key': 'warn',
      'svelte/prefer-svelte-reactivity': 'warn',
      // Existing codebase has a handful of these in regex/template patterns;
      // warn so new ones are visible without blocking.
      'no-useless-escape': 'warn',
      '@typescript-eslint/no-unused-expressions': 'warn',
      'svelte/no-unused-svelte-ignore': 'warn',
      'svelte/prefer-writable-derived': 'warn',
    },
  },
  {
    // .svelte.ts files (Svelte 5 runes in plain TS modules) — ESLint doesn't
    // auto-apply the TS parser to non-standard extensions, so do it explicitly.
    files: ['**/*.svelte.ts'],
    languageOptions: {
      parser: ts.parser,
    },
  },
  {
    // Svelte files: runes mode, ignore a11y noise from dialog overlay patterns.
    files: ['**/*.svelte'],
    languageOptions: {
      parserOptions: {
        parser: ts.parser,
      },
      globals: {
        // Injected by Vite define in vite.config.ts
        __APP_VERSION__: 'readonly',
      },
    },
    rules: {
      // Dialogs use click+keydown on overlay divs; the a11y rule fires false
      // positives on the capture-phase keydown pattern.
      'a11y/click-events-have-key-events': 'off',
      'a11y/no-static-element-interactions': 'off',
      // Svelte $props() destructuring MUST use let (reactive proxies), but
      // ESLint sees the bindings as never-reassigned. False positive in runes mode.
      'prefer-const': 'off',
    },
  },
  {
    // Config files and scripts: allow console freely.
    files: ['*.config.ts', '*.config.js', 'scripts/**'],
    rules: {
      'no-console': 'off',
    },
  },
  {
    ignores: [
      'dist/**',
      'build/**',
      'target/**',
      'src-tauri/**',
      'node_modules/**',
      '.svelte-kit/**',
      '.worktrees/**',
      'vite-plugins/**',
    ],
  },
];
