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
      "packages/github-webhooks/dist/**",
      "packages/github-webhooks/.wrangler/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    // The browser verification scripts run in Node and hand functions to the
    // page (`page.evaluate`, init scripts), which run in the browser: both
    // sets of globals are theirs.
    files: ["verification/**/*.mjs"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
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
    // Inlined into the load fallback and the dev-server status page. The
    // entry is `wireStartupActions`, which those scripts call after the paste.
    files: ["src/host/startup-actions.js"],
    languageOptions: {
      globals: { ...globals.browser },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": "off",
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
    // The desktop composer says nothing of its own: a control with something
    // to say says it as its tooltip (`ComposerNote` in the file). A status
    // line between the draft and its controls is what these refuse, in any
    // of the ways one has been written there; `composer.test.tsx` checks the
    // rendered form holds nothing but the draft and its row.
    // Its one voice is `composer-spoken.tsx`: read aloud beside it, never drawn.
    files: ["src/desktop/ui/composer.tsx"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            ...tauriSeam["no-restricted-imports"][1].paths,
            {
              name: "@nessa-ui/react/status-label",
              message:
                "The composer has no status line. Give the control it concerns a ComposerNote: its tooltip says it.",
            },
          ],
          patterns: tauriSeam["no-restricted-imports"][1].patterns,
        },
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "JSXAttribute[name.name='role'][value.value=/^(status|alert|log|note)$/]",
          message:
            "The composer has no status line. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
        {
          selector:
            "JSXAttribute[name.name='role'] Literal[value=/^(status|alert|log|note)$/]",
          message:
            "The composer has no status line. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
        {
          selector: "JSXOpeningElement[name.name='output']",
          message:
            "The composer has no status line. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
        {
          selector: "TemplateElement[value.raw=/notice/i]",
          message:
            "The composer has no notices. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
        {
          selector: "JSXAttribute[name.name='aria-live']",
          message:
            "The composer has no status line. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
        {
          selector: "Literal[value=/notice/i]",
          message:
            "The composer has no notices. Give the control it concerns a ComposerNote: its tooltip says it.",
        },
      ],
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
