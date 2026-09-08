import { forgeStore } from "../store/forgeStore";
import { runtimeStore } from "../store/runtimeStore";
import { sessionIsActive, sessionTitle, type Session } from "./types";

export type TabGround = "current" | "needs-you" | "unread" | "quiet";

export function sessionAttention(sessionId: string): { wants_you: boolean; unread: boolean } {
  const flags = forgeStore.session_attention[sessionId];
  return flags ?? { wants_you: false, unread: false };
}

export function tabGround(session: Session, activeId: string | null): TabGround {
  if (session.id === activeId) {
    return "current";
  }
  const { wants_you, unread } = sessionAttention(session.id);
  if (wants_you) {
    return "needs-you";
  }
  if (unread) {
    return "unread";
  }
  return "quiet";
}

export function shellTabLabel(session: Session, sessions: Session[]): string {
  if (session.title.user?.trim()) {
    return session.title.user;
  }
  const shells = sessions.filter((item) => item.agent_provider_id == null);
  const number = shells.filter((other) => other.id < session.id).length + 1;
  return `Terminal ${number}`;
}

export function sessionTabLabel(session: Session, sessions: Session[]): string {
  if (session.agent_provider_id != null) {
    return sessionTitle(session);
  }
  return shellTabLabel(session, sessions);
}

export function needsYouSessions(): Session[] {
  return forgeStore.sessions.filter((session) => {
    const { wants_you } = sessionAttention(session.id);
    return wants_you && session.id !== runtimeStore.activeSession;
  });
}

/** How long a session has been waiting: `now`, `3m`, `2h`, `4d`. */
export function waitedFor(session: Session, now = Date.now()): string {
  const stamp = session.last_activity_at ?? session.created_at;
  const then = Date.parse(stamp);
  if (Number.isNaN(then)) return "";
  return shortDuration(Math.max(0, now - then));
}

export function shortDuration(elapsedMs: number): string {
  const secs = Math.floor(elapsedMs / 1000);
  if (secs < 60) return "now";
  if (secs <= 3599) return `${Math.floor(secs / 60)}m`;
  if (secs <= 86_399) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86_400)}d`;
}

export function inactiveSessions(): Session[] {
  return forgeStore.sessions.filter((session) => !sessionIsActive(session.state));
}
