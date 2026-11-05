import { SECTION } from "./languages";

/** The VS Code Settings search that lists every generated native option. */
export function formatSettingsQuery(): string {
  return `@ext:fredrir.dotfmt ${SECTION}.format`;
}

/**
 * The configuration file `dotfmt.openConfig` should open: the nearest local
 * file when it exists, otherwise an existing global file, otherwise the local
 * path so the caller can offer to create it.
 */
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
