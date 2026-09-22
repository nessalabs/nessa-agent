import js from "@eslint/js"
import reactHooks from "eslint-plugin-react-hooks"
import globals from "globals"
import tseslint from "typescript-eslint"

import { composerNotices } from "./scripts/eslint/composer-notices.mjs"
import { inheritedLookups } from "./scripts/eslint/inherited-lookups.mjs"

/** This repository's own rules. Each one is documented where it is written. */
const nessa = {
  rules: {
    "composer-notices": composerNotices,
    "inherited-lookups": inheritedLookups,
  },
}

const tauriSeam = {
  "no-restricted-imports": [
    "error",
    {
      paths: [
        {
          name: "@tauri-apps/api",
          message: "Talk to the host through src/host/window.ts.",
        },
      ],
      patterns: [
        {
          group: ["@tauri-apps/*"],
          message: "Talk to the host through src/host/window.ts.",
        },
      ],
    },
  ],
}

export default tseslint.config(
  {
    ignores: [
      "dist",
      "src-tauri",
      ".vendor",
      "node_modules",
      "scripts",
      "packages/**/src/generated/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}", "packages/**/*.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser },
    },
    plugins: {
      "react-hooks": reactHooks,
      nessa,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      // `in` finds what a table inherits as well as what it holds, so a key
      // that arrived from outside the process reads a function off
      // `Object.prototype` and passes the guard. See the rule for why this is
      // the one shape of #110 worth a rule.
      "nessa/inherited-lookups": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-non-null-assertion": "error",
      ...tauriSeam,
    },
  },
  {
    files: ["src/host/window.ts"],
    rules: {
      "no-restricted-imports": "off",
    },
  },
  {
    files: ["src/**/*.test.ts", "packages/**/*.test.ts"],
    rules: {
      "@typescript-eslint/no-non-null-assertion": "off",
    },
  },
  {
    files: ["packages/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-imports": "off",
      "react-hooks/rules-of-hooks": "off",
      "react-hooks/exhaustive-deps": "off",
    },
  },
  {
    files: ["src/panel/ui/app.tsx"],
    rules: {
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "CallExpression[callee.property.name='useEffect'], CallExpression[callee.name='useEffect']",
          message: "app.tsx renders the panel chrome. Effects belong in a hook.",
        },
      ],
      // The composer's notices have one box, and it owns both the room they
      // take and the order they are said in. See the rule for why this is here
      // rather than in `scripts/architecture`.
      "nessa/composer-notices": "error",
    },
  },
)
