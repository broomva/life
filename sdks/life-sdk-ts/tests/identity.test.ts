/**
 * `life.v1.Identity` service unit tests.
 */

import { describe, expect, it } from "vitest";
import { LifeClient } from "../src/index.js";
import { FakeGateway, expectAuthHeader } from "./_helpers.js";

describe("IdentityClient", () => {
  it("whoami sends an empty request body and returns the Account", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Identity/Me",
        handler: async ({ body }) => {
          // empty proto message → empty JSON object
          expect(body).toEqual({});
          return {
            unary: {
              userId: "u-42",
              handle: "alice",
              displayName: "Alice",
              tier: "tier-1",
              createdAt: "2026-04-30T12:00:00Z",
            },
          };
        },
      },
    ]);
    const life = new LifeClient({
      baseUrl: "https://gw.test",
      getAuthToken: async () => "tok-id",
      fetch: gw.fetch,
    });
    const me = await life.identity.whoami();
    expect(me.userId).toBe("u-42");
    expect(me.tier).toBe("tier-1");
    expectAuthHeader(gw.recordedRequests[0]!.headers, "tok-id");
  });

  it("listSessions returns the SessionList shape", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Identity/ListSessions",
        handler: async () => ({
          unary: {
            sessions: [
              { sid: { value: "sid-1" }, projectId: "p", openedAt: "2026-04-30T12:00:00Z", label: "demo" },
            ],
          },
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const list = await life.identity.listSessions({ includeClosed: false });
    expect(list.sessions).toHaveLength(1);
    expect(list.sessions[0]?.sid.value).toBe("sid-1");
  });

  it("updateProfile is unary; revokeSession returns IdentityEmpty", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Identity/UpdateProfile",
        handler: async ({ body }) => {
          expect((body as { profile: { bio: string } }).profile.bio).toBe("hi");
          return { unary: { userId: "u-1", profile: { bio: "hi" } } };
        },
      },
      {
        path: "/life.v1.Identity/RevokeSession",
        handler: async ({ body }) => {
          expect((body as { sid: { value: string } }).sid.value).toBe("sid-2");
          return { unary: {} };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const acct = await life.identity.updateProfile({ profile: { bio: "hi" } });
    expect(acct.profile?.bio).toBe("hi");
    await life.identity.revokeSession({ sid: { value: "sid-2" } });
  });

  it("does not attach Authorization when getAuthToken is undefined", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Identity/Me",
        handler: async () => ({ unary: { userId: "anon" } }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    await life.identity.whoami();
    expect(gw.recordedRequests[0]?.headers["authorization"]).toBeUndefined();
  });
});
