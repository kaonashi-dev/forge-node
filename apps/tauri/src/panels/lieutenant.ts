/** The part of a job stream that belongs in a conversational answer. */
export function answerLines(output: readonly (string | undefined)[]): string[] {
  const lines: string[] = [];
  let lookedAt = 0;

  for (const line of output) {
    if (line === undefined) continue;
    if (line.startsWith("assistant  ")) {
      const rest = line.slice("assistant  ".length);
      if (!rest.startsWith("→")) {
        lines.push(rest);
        continue;
      }
    }
    if (line.startsWith("result  ")) {
      lines.push(line.slice("result  ".length));
      continue;
    }
    if (line.startsWith("$") || line.trimStart().startsWith("⤷")) {
      lookedAt += 1;
    }
  }

  if (lookedAt > 0) lines.push(`(${lookedAt} command(s) run while answering)`);
  return lines;
}

export function jobIsFinal(state: string): boolean {
  return state === "Succeeded" || state === "Failed" || state === "Cancelled";
}
