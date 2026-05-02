/**
 * `life.v1.Identity` service client.
 *
 * @see proto/life/v1/identity.proto
 */

import type { Transport, TransportCallOptions } from "../transport.js";
import type {
  Account,
  IdentityEmpty,
  IdentitySessionRef,
  ListSessionsReq,
  SessionList,
  UpdateProfileReq,
} from "../proto/identity.js";

const SERVICE = "life.v1.Identity";

export class IdentityClient {
  constructor(private readonly transport: Transport) {}

  /**
   * Resolve the calling identity. The bearer token determines the
   * returned `Account`.
   */
  whoami(opts?: TransportCallOptions): Promise<Account> {
    return this.transport.unary<IdentityEmpty, Account>(SERVICE, "Me", {}, opts);
  }

  /**
   * Update the caller's profile fields. Server responds with the
   * post-update `Account`.
   */
  updateProfile(req: UpdateProfileReq, opts?: TransportCallOptions): Promise<Account> {
    return this.transport.unary<UpdateProfileReq, Account>(
      SERVICE,
      "UpdateProfile",
      req,
      opts,
    );
  }

  /**
   * List the caller's sessions.
   */
  listSessions(req: ListSessionsReq, opts?: TransportCallOptions): Promise<SessionList> {
    return this.transport.unary<ListSessionsReq, SessionList>(
      SERVICE,
      "ListSessions",
      req,
      opts,
    );
  }

  /**
   * Revoke an active session.
   */
  revokeSession(
    req: IdentitySessionRef,
    opts?: TransportCallOptions,
  ): Promise<IdentityEmpty> {
    return this.transport.unary<IdentitySessionRef, IdentityEmpty>(
      SERVICE,
      "RevokeSession",
      req,
      opts,
    );
  }
}
