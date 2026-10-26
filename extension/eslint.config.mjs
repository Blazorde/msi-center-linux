// SPDX-License-Identifier: GPL-3.0-or-later
// ESLint flat config for GJS (GNOME Shell 51, ESM). Used by scripts/ext-check.sh when ESLint is installed
// locally (`npx --no-install eslint`); no network is ever required. Style follows gnome-shell's own lint rules.
export default [
    {
        files: ['msi-center@blazorde/**/*.js', 'tests/**/*.mjs'],
        languageOptions: {
            ecmaVersion: 2024,
            sourceType: 'module',
            globals: {
                // GJS / GNOME Shell globals
                global: 'readonly', log: 'readonly', logError: 'readonly', print: 'readonly',
                printerr: 'readonly', imports: 'readonly', console: 'readonly',
                TextEncoder: 'readonly', TextDecoder: 'readonly',
                setTimeout: 'readonly', clearTimeout: 'readonly',
            },
        },
        rules: {
            'no-unused-vars': ['error', {argsIgnorePattern: '^_', varsIgnorePattern: '^_'}],
            'no-undef': 'error',
            'prefer-const': 'error',
            'no-var': 'error',
            'eqeqeq': ['error', 'always'],
            'curly': ['error', 'multi-or-nest', 'consistent'],
            'indent': ['error', 4, {SwitchCase: 0}],
            'quotes': ['error', 'single', {avoidEscape: true}],
            'semi': ['error', 'always'],
            'comma-dangle': ['error', 'always-multiline'],
            'object-curly-spacing': ['error', 'never'],
        },
    },
];
