import { For, Show, createSignal } from "solid-js";
import {
  AlertDialog,
  Badge,
  Button,
  Checkbox,
  Dialog,
  Disclosure,
  Menu,
  RadioGroup,
  SearchField,
  Breadcrumbs,
  Kbd,
  ListCard,
  Progress,
  Select,
  Skeleton,
  Switch,
  TextField,
  Tooltip,
  toast,
} from "../ui";
import { Icon, WorkMarker } from "./icons";
import { applyThemeBase, themeBase } from "./ThemeProvider";
import { palettes, type ThemeBaseId } from "./tokens";
import { DENSITIES, applyDensity, type Density } from "./density";

/**
 * `?gallery` — the base layer on one screen.
 *
 * Every swatch is painted with `var(--forge-*)` (or, after F1, a semantic
 * token), so it reflects whatever the pipeline currently emits: the same page
 * is the eyeball review for F0's colours, F1's semantic layer and F2's denser
 * scale without a line of it changing between phases. It is mounted only when
 * the URL carries `?gallery`, never bundled into the shell's own tree.
 */

const bases = Object.keys(palettes) as ThemeBaseId[];

/** Read a resolved custom-property value, for the caption under a swatch. */
function resolved(name: string): string {
  if (typeof window === "undefined") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function Section(props: { title: string; children: unknown }) {
  return (
    <section style={{ display: "flex", "flex-direction": "column", gap: "10px" }}>
      <h2
        style={{
          margin: "0",
          "font-size": "var(--forge-text-xs)",
          "letter-spacing": "0.08em",
          "text-transform": "uppercase",
          color: "var(--forge-faint)",
        }}
      >
        {props.title}
      </h2>
      {props.children as never}
    </section>
  );
}

function Swatch(props: { token: string; label?: string; ring?: boolean }) {
  // Re-read on every base change: `applyTheme` rewrites the inline vars before
  // `themeBase` settles, so touching the signal here keeps the caption honest.
  const value = () => {
    themeBase();
    return resolved(props.token);
  };
  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "4px", "min-width": "0" }}>
      <div
        style={{
          height: "44px",
          "border-radius": "var(--forge-radius-sm)",
          background: `var(${props.token})`,
          border: props.ring ? "1px solid var(--forge-border)" : "none",
        }}
      />
      <div style={{ "font-size": "11px", color: "var(--forge-muted)" }}>
        {props.label ?? props.token.replace("--forge-", "")}
      </div>
      <div
        style={{
          "font-size": "10px",
          color: "var(--forge-faint)",
          "font-family": "var(--forge-mono)",
        }}
      >
        {value()}
      </div>
    </div>
  );
}

function Grid(props: { children: unknown }) {
  return (
    <div
      style={{
        display: "grid",
        "grid-template-columns": "repeat(auto-fill, minmax(110px, 1fr))",
        gap: "12px",
      }}
    >
      {props.children as never}
    </div>
  );
}

/** A saturated fill with the semantic foreground `on()` picked painted over it. */
function FillChip(props: { fill: string; fg: string; label: string }) {
  return (
    <div
      style={{
        height: "44px",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "border-radius": "var(--radius-md)",
        background: `var(${props.fill})`,
        color: `var(${props.fg})`,
        "font-size": "12px",
        "font-weight": "600",
      }}
    >
      {props.label}
    </div>
  );
}

export function Gallery() {
  const [density, setDensity] = createSignal<Density>("default");
  return (
    <div
      style={{
        height: "100%",
        overflow: "auto",
        background: "var(--forge-bg)",
        color: "var(--forge-text)",
        "font-family": "var(--forge-sans)",
      }}
    >
      <header
        style={{
          position: "sticky",
          top: "0",
          "z-index": "1",
          display: "flex",
          "align-items": "center",
          gap: "12px",
          padding: "12px 24px",
          background: "var(--forge-rail)",
          "border-bottom": "1px solid var(--forge-border)",
        }}
      >
        <strong style={{ "font-size": "13px" }}>Design gallery</strong>
        {/* T5: the header is the review surface's own control panel — every
            base and every density, so a change is compared rather than
            remembered. `system` is not here: this page is for looking at a
            specific base, and "whichever the OS says" is the one answer that
            cannot be looked at. */}
        <div style={{ display: "flex", gap: "6px" }}>
          <For each={bases}>
            {(id) => (
              <Button
                variant="secondary"
                size="sm"
                selected={themeBase() === id}
                onClick={() => applyThemeBase(id)}
              >
                {id}
              </Button>
            )}
          </For>
        </div>
        <div style={{ display: "flex", gap: "6px", "margin-left": "auto" }}>
          <For each={DENSITIES}>
            {(id) => (
              <Button
                variant="secondary"
                size="sm"
                selected={density() === id}
                onClick={() => {
                  setDensity(id);
                  applyDensity(id);
                }}
              >
                {id}
              </Button>
            )}
          </For>
        </div>
      </header>

      <div
        style={{
          display: "flex",
          "flex-direction": "column",
          gap: "28px",
          padding: "24px",
          "max-width": "960px",
        }}
      >
        <Section title="Surfaces">
          <Grid>
            <Swatch token="--forge-bg" ring />
            <Swatch token="--forge-rail" ring />
            <Swatch token="--forge-sidebar" ring />
            <Swatch token="--forge-surface" ring />
            <Swatch token="--forge-surface-hi" ring />
            <Swatch token="--forge-editor" ring />
            <Swatch token="--forge-step" />
          </Grid>
        </Section>

        <Section title="Foreground">
          <Grid>
            <Swatch token="--forge-text" ring />
            <Swatch token="--forge-muted" ring />
            <Swatch token="--forge-faint" ring />
          </Grid>
        </Section>

        <Section title="Borders & interaction">
          <Grid>
            <Swatch token="--forge-border" ring />
            <Swatch token="--forge-border-hi" ring />
            <Swatch token="--forge-hover" ring />
            <Swatch token="--forge-selected" ring />
            <Swatch token="--forge-pressed" ring />
          </Grid>
        </Section>

        <Section title="Accent & status — fill with derived foreground">
          <Grid>
            <FillChip fill="--accent-solid" fg="--accent-fg" label="Accent" />
            <FillChip fill="--attention-solid" fg="--attention-fg" label="Attention" />
            <FillChip fill="--success-solid" fg="--success-fg" label="Success" />
            <FillChip fill="--danger-solid" fg="--danger-fg" label="Danger" />
            <FillChip fill="--warning-solid" fg="--warning-fg" label="Warning" />
            <FillChip fill="--forge-blue" fg="--fg-default" label="Info" />
          </Grid>
        </Section>

        <Section title="Radius">
          <div style={{ display: "flex", gap: "16px", "align-items": "flex-end" }}>
            <For
              each={
                [
                  ["--forge-radius-xs", "xs"],
                  ["--forge-radius-sm", "sm"],
                  ["--forge-radius-md", "md"],
                  ["--forge-radius-lg", "lg"],
                  ["--forge-radius-xl", "full"],
                ] as [string, string][]
              }
            >
              {([token, label]) => (
                <div
                  style={{
                    display: "flex",
                    "flex-direction": "column",
                    gap: "6px",
                    "align-items": "center",
                  }}
                >
                  <div
                    style={{
                      width: "56px",
                      height: "56px",
                      background: "var(--forge-surface)",
                      border: "1px solid var(--forge-border-hi)",
                      "border-radius": `var(${token})`,
                    }}
                  />
                  <span style={{ "font-size": "11px", color: "var(--forge-muted)" }}>{label}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Control heights">
          <div style={{ display: "flex", gap: "12px", "align-items": "center" }}>
            <For
              each={
                [
                  ["--forge-control-xs", "xs"],
                  ["--forge-control-sm", "sm"],
                  ["--forge-control-md", "md"],
                  ["--forge-control-lg", "lg"],
                ] as [string, string][]
              }
            >
              {([token, label]) => (
                <div
                  style={{
                    height: `var(${token})`,
                    padding: "0 14px",
                    display: "flex",
                    "align-items": "center",
                    "border-radius": "var(--forge-radius-xs)",
                    background: "var(--forge-surface)",
                    border: "1px solid var(--forge-border)",
                    "font-size": "12px",
                  }}
                >
                  {label}
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Type scale">
          <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
            <For
              each={
                [
                  ["--forge-text-xs", "xs — metadata"],
                  ["--forge-text-sm", "sm — the chrome's default"],
                  ["--forge-text-md", "md — a body line"],
                  ["--forge-text-lg", "lg — a panel heading"],
                  ["--forge-text-xl", "xl — the one big heading"],
                ] as [string, string][]
              }
            >
              {([token, label]) => <div style={{ "font-size": `var(${token})` }}>{label}</div>}
            </For>
          </div>
        </Section>

        <Section title="Elevation">
          <div style={{ display: "flex", gap: "24px" }}>
            <For each={["--forge-shadow-sm", "--forge-shadow-md", "--forge-shadow-lg"]}>
              {(token) => (
                <div
                  style={{
                    width: "120px",
                    height: "72px",
                    "border-radius": "var(--forge-radius-md)",
                    background: "var(--forge-surface)",
                    "box-shadow": `var(${token})`,
                  }}
                />
              )}
            </For>
          </div>
        </Section>

        <Primitives />
      </div>
    </div>
  );
}

function Primitives() {
  const [text, setText] = createSignal("");
  const [search, setSearch] = createSignal("bran");
  const [dialog, setDialog] = createSignal(false);
  const [alert, setAlert] = createSignal(false);
  const [checked, setChecked] = createSignal(true);
  const [choice, setChoice] = createSignal("all");
  const [picked, setPicked] = createSignal("claude");
  const [autosave, setAutosave] = createSignal(true);
  const row = {
    display: "flex",
    gap: "12px",
    "align-items": "center",
    "flex-wrap": "wrap",
  } as const;
  return (
    <>
      <Section title="Buttons — hierarchy">
        <div style={row}>
          <Button variant="primary">Primary</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button variant="danger-ghost">Danger ghost</Button>
        </div>
        <div style={row}>
          <Button variant="primary" loading>
            Loading
          </Button>
          <Button variant="secondary" selected>
            Selected
          </Button>
          <Button variant="secondary" disabled>
            Disabled
          </Button>
          <Button variant="primary" disabled>
            Disabled
          </Button>
        </div>
        <div style={row}>
          <Button variant="secondary" size="xs">
            xs
          </Button>
          <Button variant="secondary" size="sm">
            sm
          </Button>
          <Button variant="secondary" size="md">
            md
          </Button>
          <Button variant="secondary" size="lg">
            lg
          </Button>
        </div>
      </Section>

      <Section title="Fields">
        <div style={{ display: "flex", gap: "16px", "flex-wrap": "wrap", "max-width": "560px" }}>
          <TextField label="Default" value={text()} onChange={setText} placeholder="Type…" />
          <TextField
            label="Invalid"
            value={text()}
            onChange={setText}
            invalid
            errorMessage="Something is off"
          />
          <div style={{ "min-width": "200px" }}>
            <SearchField value={search()} onChange={setSearch} onClear={() => setSearch("")} />
          </div>
        </div>
      </Section>

      {/*
       * The surfaces the motion is actually on.
       *
       * Every one of these was invisible to this page before: a menu that
       * grows out of its trigger, a chevron that turns, a tick that pops, a
       * disclosure that collapses. A gallery that could not open one of them
       * could not be used to review any of it.
       */}
      <Section title="Interactions — open one of each to see the motion">
        <div style={row}>
          <Menu
            triggerLabel="Open menu"
            trigger={<span>Menu ▾</span>}
            triggerClass="forge-control forge-control-md forge-btn forge-btn-secondary"
            items={[
              { kind: "heading", label: "Session" },
              { kind: "item", label: "Refresh status", icon: "refresh", run: () => undefined },
              { kind: "item", label: "Copy path", icon: "copy", run: () => undefined },
              { kind: "rule" },
              {
                kind: "submenu",
                label: "Open in",
                icon: "folder-open",
                items: [{ kind: "item", label: "Finder", run: () => undefined }],
              },
              {
                kind: "item",
                label: "Remove",
                icon: "trash",
                destructive: true,
                run: () => undefined,
              },
            ]}
          />
          <div style={{ "min-width": "180px" }}>
            <Select
              aria-label="Provider"
              value={picked()}
              onChange={setPicked}
              options={[
                { value: "claude", label: "Claude" },
                { value: "codex", label: "Codex" },
                { value: "opencode", label: "opencode" },
              ]}
            />
          </div>
          <Tooltip label="A tooltip only fades — a scale would read as a twitch">
            <Button variant="ghost" iconLeading={<Icon name="search" size={14} />}>
              Hover me
            </Button>
          </Tooltip>
        </div>

        <div style={row}>
          <Checkbox checked={checked()} onChange={setChecked} label="Checkbox" />
          <RadioGroup
            label="Filter"
            hideLabel={false}
            orientation="horizontal"
            value={choice()}
            onChange={setChoice}
            itemClass="forge-chip"
            options={[
              { value: "all", label: "All" },
              { value: "mine", label: "Mine" },
              { value: "open", label: "Open" },
            ]}
          />
          <Badge tone="good">ready</Badge>
          <Badge tone="warn">draft</Badge>
        </div>

        <div style={row}>
          <For each={["running", "working", "starting", "needs-you", "failed"] as const}>
            {(work) => (
              <span style={{ display: "inline-flex", "align-items": "center", gap: "4px" }}>
                <WorkMarker work={work} />
                <span style={{ "font-size": "11px", color: "var(--fg-muted)" }}>{work}</span>
              </span>
            )}
          </For>
        </div>

        <div style={{ "max-width": "420px" }}>
          <Disclosure summary={<span>A disclosure — this one animates its height</span>}>
            <p style={{ margin: "0", color: "var(--fg-muted)" }}>
              Kobalte publishes the measured height as `--kb-collapsible-content-height`, which is
              what the keyframe interpolates to. `auto` does not animate.
            </p>
          </Disclosure>
        </div>

        <div style={{ display: "flex", "flex-direction": "column", "max-width": "420px" }}>
          <For each={["A row", "Another row", "A third"]}>
            {(label) => (
              <button type="button" class="forge-row empty-center-row">
                <Icon name="square-terminal" class="forge-icon-muted" size={14} />
                <span>{label}</span>
              </button>
            )}
          </For>
        </div>
      </Section>

      <Section title="Dialogs">
        <div style={row}>
          <Button variant="secondary" onClick={() => setDialog(true)}>
            Open dialog
          </Button>
          <Button variant="danger-ghost" onClick={() => setAlert(true)}>
            Open alert
          </Button>
        </div>
        <Show when={dialog()}>
          <Dialog
            title="A dialog"
            description="Sized from a token, body scrolls, animates in and out."
            align="center"
            size="md"
            onDismiss={() => setDialog(false)}
            footer={
              <>
                <Button variant="secondary" onClick={() => setDialog(false)}>
                  Cancel
                </Button>
                <Button variant="primary" onClick={() => setDialog(false)}>
                  Confirm
                </Button>
              </>
            }
          >
            <p class="forge-dialog-copy">
              The header, this body and the footer are three regions; only the body scrolls.
            </p>
          </Dialog>
        </Show>
        <Show when={alert()}>
          <AlertDialog
            title="Delete this?"
            description="AlertDialog: the destructive role, no dismiss on click-outside."
            onDismiss={() => setAlert(false)}
            footer={
              <>
                <Button variant="secondary" onClick={() => setAlert(false)}>
                  Cancel
                </Button>
                <Button variant="danger" onClick={() => setAlert(false)}>
                  Delete
                </Button>
              </>
            }
          />
        </Show>
      </Section>

      {/* T5: everything §5.3 added, on the same page as everything before it,
          so "does the new switch look like the rest of the app" is a question
          that can be answered by looking rather than by remembering. */}
      <Section title="Booleans, progress and placeholders">
        <div
          style={{ display: "flex", "flex-direction": "column", gap: "12px", "max-width": "360px" }}
        >
          <Switch
            label="Autosave"
            description="Write on blur, and after a second of stillness."
            checked={autosave()}
            onChange={setAutosave}
          />
          <Switch label="Disabled, on" checked disabled onChange={() => undefined} />
          <Switch label="Small" size="sm" checked={autosave()} onChange={setAutosave} />
          <Progress label="Reading the checkout" />
          <Progress label="Steps" value={40} detail="2 of 5" />
          <Skeleton label="Loading the file tree" rows={4} />
        </div>
      </Section>

      <Section title="Chords, crumbs and cards">
        <div
          style={{ display: "flex", "flex-direction": "column", gap: "12px", "max-width": "460px" }}
        >
          <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
            <Kbd action="open_command_palette" />
            <Kbd action="new_terminal" />
            <Kbd action="save_file" />
          </div>
          <Breadcrumbs
            label="File path"
            crumbs={[
              { label: "src", value: "src" },
              { label: "workbench", value: "src/workbench" },
              { label: "EditorView.tsx", value: "src/workbench/EditorView.tsx" },
            ]}
            onChoose={() => undefined}
          />
          <ListCard
            glyph={<Icon name="git-pull-request" size={14} />}
            title="Replace the textarea editor with CodeMirror"
            aside={<Badge tone="good">open</Badge>}
            meta={
              <>
                <span>#128</span>
                <span>2 files</span>
                <span>opened 3h ago</span>
              </>
            }
            actions={
              <>
                <Button variant="secondary" size="xs">
                  Check out
                </Button>
                <Button variant="ghost" size="xs">
                  Open on GitHub
                </Button>
              </>
            }
          >
            <p style={{ margin: "0", color: "var(--fg-muted)" }}>
              The body slot, for whatever a particular list wants under the metadata.
            </p>
          </ListCard>
          <Button
            variant="secondary"
            size="sm"
            onClick={() =>
              toast({ title: "Saved", detail: "src/theme/Gallery.tsx", tone: "success" })
            }
          >
            Fire a toast
          </Button>
        </div>
      </Section>
    </>
  );
}
