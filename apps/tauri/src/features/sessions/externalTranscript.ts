import type { SessionTranscript } from "../../contracts/workbench";
import type { ExternalTranscript } from "../../contracts/runtime";

/**
 * A discovered run's capture as the handoff dialog's shape.
 *
 * A terminal capture counts `lines`; a transcript file has no terminal rows, so
 * the daemon counts `turns`. Mapping them here keeps the dialog ignorant of
 * where its capture came from.
 */
export function asSessionTranscript(transcript: ExternalTranscript): SessionTranscript {
  return {
    session_id: transcript.session_id,
    text: transcript.text,
    lines: transcript.turns,
    truncated: transcript.truncated,
  };
}
