/**
 * `life.v1.Identity` proto types.
 *
 * Hand-curated TypeScript mirror of `proto/life/v1/identity.proto`.
 *
 * @see proto/life/v1/identity.proto
 */

import type { SessionId } from "./aios.js";
import type { Timestamp } from "./timestamp.js";

export interface Profile {
  bio?: string;
  avatarBlobRef?: Uint8Array;
  preferences?: Record<string, string>;
}

export interface Account {
  userId: string;
  handle?: string;
  displayName?: string;
  email?: string;
  tier?: string;
  createdAt?: Timestamp;
  profile?: Profile;
}

export interface UpdateProfileReq {
  profile: Profile;
}

export interface ListSessionsReq {
  includeClosed?: boolean;
  limit?: number;
}

export interface SessionDescriptor {
  sid: SessionId;
  projectId?: string;
  openedAt?: Timestamp;
  closedAt?: Timestamp;
  label?: string;
}

export interface SessionList {
  sessions: SessionDescriptor[];
}

export interface IdentityEmpty {}

export interface IdentitySessionRef {
  sid: SessionId;
}
