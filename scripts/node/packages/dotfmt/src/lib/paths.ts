import os from "node:os";
import path from "node:path";
import { CONFIG_NAME } from "./languages";

/** Whether `child` is `parent` or inside it, compared by path segments rather than string prefix. */
export function isWithin(parent: string, child: string): boolean {
  const relative = path.relative(path.resolve(parent), path.resolve(child));
  return (
    relative === "" ||
    (relative !== ".." &&
      !relative.startsWith(`..${path.sep}`) &&
      !path.isAbsolute(relative))
  );
}

/** Expands a leading `~` to the home directory. */
export function expandHome(value: string, home = os.homedir()): string {
  return value.replace(/^~(?=$|[\\/])/, home);
}

/** Shortens absolute paths under the home directory to `~/...`. */
export function tildify(
  value: string,
  home = process.env.HOME ?? os.homedir(),
): string {
  if (!home) {
    return value;
  }
  if (value === home) {
    return "~";
  }
  return value.startsWith(`${home}${path.sep}`)
    ? `~${value.slice(home.length)}`
    : value;
}

/** The nearest `dotfmt.dotfile` between `file` and `root`, never outside `root`. */
export function findConfig(
  file: string,
  root: string,
  exists: (candidate: string) => boolean,
): string | undefined {
  const resolvedRoot = path.resolve(root);
  let directory = path.dirname(path.resolve(file));
  while (isWithin(resolvedRoot, directory)) {
    const candidate = path.join(directory, CONFIG_NAME);
    if (exists(candidate)) {
      return candidate;
    }
    if (directory === resolvedRoot) {
      break;
    }
    const parent = path.dirname(directory);
    if (parent === directory) {
      break;
    }
    directory = parent;
  }
  return undefined;
}
