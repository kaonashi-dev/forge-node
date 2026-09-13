/** Whether the Features panel should ask the daemon for the list. */

export function shouldLoadFeatureList(state: {
  project: string | null;
  loadingList: boolean;
  listReady: boolean;
  listError: string | null;
}): boolean {
  return Boolean(state.project) && !state.loadingList && !state.listReady && !state.listError;
}

/** Whether a feature tab should ask the daemon for detail. */

export function shouldLoadFeatureDetail(state: {
  project: string | null;
  featureId: number;
  detailId: number | null | undefined;
  loadingDetail: boolean;
  detailError: string | null;
}): boolean {
  if (!state.project || state.loadingDetail) return false;
  if (state.detailId === state.featureId) return false;
  if (state.detailError) return false;
  return true;
}
