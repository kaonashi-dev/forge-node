import { createStore } from "solid-js/store";

/** Reads in flight, by surface, so a panel can say it is working. */
export const [loading, setLoading] = createStore<Record<string, boolean>>({});
