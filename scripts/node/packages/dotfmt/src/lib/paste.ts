import { LANGUAGES } from "./languages";

export interface FormatTextParams {
  text: string;
  languageId?: string;
  uri?: string;
}

export function formatTextParams(
  text: string,
  languageId: string,
  uri: string,
): FormatTextParams | undefined {
  if (LANGUAGES.includes(languageId)) {
    return { text, languageId, uri };
  }
  return uri.startsWith("file:") ? { text, uri } : undefined;
}
