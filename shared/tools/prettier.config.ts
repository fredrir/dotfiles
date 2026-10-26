import { type Config } from "prettier";

const config: Config = {
  singleQuote: false,
  semi: true,
  trailingComma: "all",
  plugins: ["prettier-plugin-organize-imports", "prettier-plugin-tailwindcss"],
};

export default config;
