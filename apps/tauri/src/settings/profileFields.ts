import type { AgentProfile, ProfileField } from "../runtime/types";

export type ProfileFieldValues = Record<string, string>;

export function newAgentProfile(
  providerId: string,
  id = globalThis.crypto.randomUUID(),
  createdAt = new Date().toISOString(),
): AgentProfile {
  return {
    id,
    provider_id: providerId,
    name: "",
    executable: null,
    args: [],
    env: [],
    created_at: createdAt,
  };
}

export function profileFieldKey(field: ProfileField): string {
  return "Env" in field.effect ? `env:${field.effect.Env.name}` : `flag:${field.effect.Flag.flag}`;
}

export function splitProfileFields(
  profile: Pick<AgentProfile, "args" | "env">,
  fields: readonly ProfileField[],
): { values: ProfileFieldValues; args: string[]; env: [string, string][] } {
  const values: ProfileFieldValues = {};
  const envFields = new Map<string, string>();
  const flagFields = new Map<string, string>();
  for (const field of fields) {
    if ("Env" in field.effect) envFields.set(field.effect.Env.name, profileFieldKey(field));
    else flagFields.set(field.effect.Flag.flag, profileFieldKey(field));
  }
  const env: [string, string][] = [];
  const args: string[] = [];

  for (const [name, value] of profile.env) {
    const key = envFields.get(name);
    if (key) values[key] = value;
    else env.push([name, value]);
  }
  for (let index = 0; index < profile.args.length; index += 1) {
    const argument = profile.args[index];
    const key = flagFields.get(argument);
    if (key && index + 1 < profile.args.length) {
      values[key] = profile.args[index + 1];
      index += 1;
    } else {
      args.push(argument);
    }
  }

  return { values, args, env };
}

export function applyProfileFields(
  profile: AgentProfile,
  fields: readonly ProfileField[],
  values: ProfileFieldValues,
): AgentProfile {
  const advanced = splitProfileFields(profile, fields);
  const args = [...advanced.args];
  const env = [...advanced.env];
  for (const field of fields) {
    const value = values[profileFieldKey(field)]?.trim();
    if (!value) continue;
    if ("Env" in field.effect) env.push([field.effect.Env.name, value]);
    else args.push(field.effect.Flag.flag, value);
  }
  return { ...profile, args, env };
}
