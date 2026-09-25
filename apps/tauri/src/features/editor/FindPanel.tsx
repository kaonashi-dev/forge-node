import { createEffect, createSignal, on } from "solid-js";
import type { EditorFind, EditorFindCommand, EditorFindFlags } from "../../contracts/runtime";
import { Icon } from "../../theme/icons/index";
import { Button, IconButton, TextField } from "../../ui/index";
import { editorFind } from "./commands";
import { findCounter, findKeyCommand, flagsOf, setFind } from "./findQuery";

const TOGGLES: { flag: keyof EditorFindFlags; glyph: string; label: string }[] = [
  { flag: "case_sensitive", glyph: "Aa", label: "Match case" },
  { flag: "whole_word", glyph: "ab", label: "Match whole word" },
  { flag: "regex", glyph: ".*", label: "Use regular expression" },
];

/**
 * Find in the open file. The editor owns the query, the search and the marks;
 * this sends what was typed and draws what the editor reports back.
 *
 * The field keeps its own text while typing, because the state that echoes a
 * keystroke arrives after the next one.
 */
export function FindPanel(props: {
  session: string;
  find: () => EditorFind;
  /** Put the keyboard back in the editor once the panel closes. */
  onReturn: () => void;
}) {
  let input: HTMLInputElement | undefined;
  const [typed, setTyped] = createSignal(props.find().pattern);

  function send(command: EditorFindCommand): void {
    void editorFind(props.session, command).catch(() => undefined);
  }

  createEffect(
    on(
      () => props.find().focus,
      () => {
        setTyped(props.find().pattern);
        queueMicrotask(() => {
          input?.focus();
          input?.select();
        });
      },
    ),
  );

  function onType(value: string): void {
    setTyped(value);
    send(setFind(value, flagsOf(props.find())));
  }

  function toggle(flag: keyof EditorFindFlags): void {
    const flags = flagsOf(props.find());
    send(setFind(typed(), { ...flags, [flag]: !flags[flag] }));
  }

  function close(): void {
    send("Close");
    props.onReturn();
  }

  function onKeyDown(event: KeyboardEvent): void {
    const command = findKeyCommand(event);
    if (!command) return;
    event.preventDefault();
    event.stopPropagation();
    if (command === "Close") close();
    else send(command);
  }

  const counter = () => findCounter(props.find(), typed());

  return (
    <div class="editor-find-anchor">
      <div
        role="search"
        class="editor-find"
        aria-label="Find in file"
        onMouseDown={(event) => event.stopPropagation()}
        onWheel={(event) => event.stopPropagation()}
      >
        <TextField
          class="editor-find-field"
          size="sm"
          value={typed()}
          onChange={onType}
          aria-label="Find"
          placeholder="Find"
          invalid={props.find().error !== null}
          ref={(element) => (input = element)}
          onKeyDown={onKeyDown}
          leading={<Icon name="search" size={12} class="forge-icon-muted" />}
        />
        <span
          class="editor-find-count"
          classList={{ "editor-find-error": props.find().error !== null }}
          role="status"
          aria-live="polite"
        >
          {counter()}
        </span>
        <div class="editor-find-toggles" role="group" aria-label="Search options">
          {TOGGLES.map((toggleDef) => (
            <Button
              variant="ghost"
              size="xs"
              class="editor-find-toggle"
              aria-label={toggleDef.label}
              title={toggleDef.label}
              selected={props.find()[toggleDef.flag]}
              onClick={() => toggle(toggleDef.flag)}
            >
              {toggleDef.glyph}
            </Button>
          ))}
        </div>
        <IconButton
          label="Previous match"
          title="Previous match (⇧↵)"
          size="xs"
          disabled={props.find().total === 0}
          onClick={() => send("Previous")}
        >
          <Icon name="chevron-up" size={14} />
        </IconButton>
        <IconButton
          label="Next match"
          title="Next match (↵)"
          size="xs"
          disabled={props.find().total === 0}
          onClick={() => send("Next")}
        >
          <Icon name="chevron-down" size={14} />
        </IconButton>
        <IconButton label="Close find" title="Close (esc)" size="xs" onClick={close}>
          <Icon name="close" size={12} />
        </IconButton>
      </div>
    </div>
  );
}
