import type { ReviewPreset, ReviewSettings } from "../../contracts/workbench";
import type { ProviderInfo } from "../../contracts/runtime";
import {
  providerId,
  providerName,
  providerReviewMode,
  providerReviews,
} from "../../contracts/runtime";

/** The built-in preset with no wording of its own (`review::CUSTOM_PRESET`). */
export const CUSTOM_PRESET = "custom";

/** `app_state` keys: the last agent and the custom wording, shared by every project. */
export const REVIEW_PROVIDER_KEY = "ui.review.provider";
export const REVIEW_CUSTOM_KEY = "ui.review.custom";

export function presetKey(projectId: string): string {
  return `ui.review.preset.${projectId}`;
}

/** The remembered preset while the repository still lists it, else its default. */
export function initialPreset(settings: ReviewSettings, remembered: string | undefined): string {
  return remembered !== undefined && settings.presets.some((preset) => preset.id === remembered)
    ? remembered
    : settings.default_preset;
}

export type Reviewer = {
  /** The provider id `StartReview` takes; a review has no profile. */
  id: string;
  name: string;
  /** What the provider calls its read-only mode — "plan mode". */
  mode: string;
};

/**
 * The installed providers that can answer a Forge Review, in snapshot order.
 *
 * Narrower than the pull-request picker: the daemon reads the answer from a
 * transcript it names in advance, which only a `structured_review` provider
 * supports, and it refuses any other.
 */
export function reviewers(providers: readonly ProviderInfo[]): Reviewer[] {
  return providers.flatMap((provider) =>
    providerReviews(provider) && provider.descriptor?.structured_review
      ? [
          {
            id: providerId(provider),
            name: providerName(provider),
            mode: providerReviewMode(provider) ?? "read-only",
          },
        ]
      : [],
  );
}

/** The remembered reviewer while it is still installed, else the first one. */
export function preferredReviewer(
  candidates: readonly Reviewer[],
  remembered: string | null,
): Reviewer | null {
  return candidates.find((item) => item.id === remembered) ?? candidates[0] ?? null;
}

/** Whether the Review button can fire. */
export function readyToReview(
  reviewer: Reviewer | null,
  preset: ReviewPreset | null,
  custom: string,
): boolean {
  if (!reviewer || !preset) return false;
  return preset.id !== CUSTOM_PRESET || custom.trim().length > 0;
}

/** The line under the choices: what the button will do, or why it cannot. */
export function reviewHint(
  reviewer: Reviewer | null,
  preset: ReviewPreset | null,
  custom: string,
): string {
  if (!reviewer) return "No installed agent can run a Forge Review.";
  if (preset?.id === CUSTOM_PRESET && !custom.trim()) {
    return "Write what to ask for, and the review will start with it.";
  }
  return `${reviewer.name} reads the change in ${reviewer.mode} — it cannot write to this checkout.`;
}
