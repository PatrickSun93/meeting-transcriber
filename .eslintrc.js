module.exports = {
  root: true,
  env: {
    browser: true,
    es2022: true,
    node: true,
    jest: true
  },
  extends: ['eslint:recommended', 'plugin:react/recommended', 'plugin:react/jsx-runtime'],
  parserOptions: {
    ecmaFeatures: {
      jsx: true
    },
    ecmaVersion: 2022,
    sourceType: 'module'
  },
  settings: {
    react: {
      version: 'detect'
    }
  },
  ignorePatterns: ['src/client/build/', 'dist/', 'browser-extension/', 'desktop-app/'],
  rules: {
    'react/prop-types': 'off',
    'no-console': 'off',
    indent: ['error', 2, { SwitchCase: 1 }],
    quotes: ['error', 'single', { avoidEscape: true }],
    semi: ['error', 'always']
  }
};
