const MAX_PATH_BYTES = 4096;

export function validatePath(path: string): string | null {
  if (!path || path.startsWith("/") || /^[A-Za-z]:/.test(path)) {
    return "Enter a path relative to the workspace.";
  }
  if (path.length > MAX_PATH_BYTES) return "The path is too long.";
  for (let at = 0; at < path.length; at++) {
    const code = path.charCodeAt(at);
    if (code < 32 || code === 127) return "Paths cannot contain control characters.";
  }
  if (path.split("/").some((part) => !part || part === "." || part === ".." || part === ".git")) {
    return "Paths cannot contain empty, dot, parent, or .git components.";
  }
  if (path.length > MAX_PATH_BYTES || new TextEncoder().encode(path).length > MAX_PATH_BYTES)
    return "The path is too long.";
  return null;
}

export function validateRename(from: string, to: string): string | null {
  const error = validatePath(from) ?? validatePath(to);
  if (error) return error;
  if (from === to) return "Choose a different destination.";
  if (to.startsWith(`${from}/`)) return "A folder cannot be moved inside itself.";
  return null;
}

export function parentPath(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

export function retargetPath(path: string, from: string, to: string): string {
  return path === from || path.startsWith(`${from}/`) ? to + path.slice(from.length) : path;
}
