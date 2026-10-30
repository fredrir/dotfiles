import { defineConfig, type OxlintConfig } from "oxlint";

const config: OxlintConfig = {
  plugins: ["typescript", "react"],
  options: {
    typeAware: true,
    maxWarnings: 0,
  },

  rules: {
    "no-var": "error",
    "react/display-name": "error",
    "react/jsx-no-comment-textnodes": "error",
    "react/jsx-no-target-blank": "error",
    "react/no-unescaped-entities": "error",
    "react/no-unknown-property": "error",
    "react/rules-of-hooks": "error",
    "react/unsupported-syntax": "error",
    "typescript/ban-ts-comment": "error",
    "typescript/no-empty-object-type": "error",
    "typescript/no-explicit-any": "error",
    "typescript/no-misused-promises": "error",
    "typescript/no-namespace": "error",
    "typescript/no-require-imports": "error",
    "typescript/no-unnecessary-type-assertion": "error",
    "typescript/no-unnecessary-type-constraint": "error",
    "typescript/no-unsafe-argument": "error",
    "typescript/no-unsafe-assignment": "error",
    "typescript/no-unsafe-call": "error",
    "typescript/no-unsafe-enum-comparison": "error",
    "typescript/no-unsafe-function-type": "error",
    "typescript/no-unsafe-member-access": "error",
    "typescript/no-unsafe-return": "error",
    "typescript/only-throw-error": "error",
    "typescript/prefer-promise-reject-errors": "error",
    "typescript/require-await": "error",
    "typescript/restrict-plus-operands": "error",
    "eslint/no-unused-vars": "error",
    "eslint/no-restricted-imports": [
      "error",
      {
        patterns: [
          {
            regex: "^(\\.\\./)+packages/",
            message: "import via @archtex/<pkg> instead",
          },
        ],
      },
    ],
  },
};

export default defineConfig(config);
