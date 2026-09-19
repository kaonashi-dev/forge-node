import { createStore } from "solid-js/store";

export type ProfileSaveResult = {
  profile: string;
  error: string | null;
};

export const [profilesStore, setProfilesStore] = createStore({
  profileSave: null as ProfileSaveResult | null,
});
