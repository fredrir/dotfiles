import { SECTION } from "./languages";

export function formatSettingsQuery(): string {
  return `@ext:fredrir.dotfmt ${SECTION}.format`;
}

export function configTarget(
  local: string | undefined,
  global: string,
  exists: (file: string) => boolean,
): string {
  if (local && exists(local)) {
    return local;
  }
  if (exists(global)) {
    return global;
  }
  return local ?? global;
}
