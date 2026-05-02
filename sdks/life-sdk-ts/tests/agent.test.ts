/**
 * `life.v1.Agent` service unit tests against a fake Connect gateway.
 */

import { describe, expect, it } from "vitest";
import { LifeClient, AuthError } from "../src/index.js";
import { FakeGateway, expectAuthHeader } from "./_helpers.js";

describe("AgentClient", () => {
  it("createSession sends Bearer token + decodes Session", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/CreateSession",
        handler: async () => ({
          unary: {
            sid: { value: "sid-fake-1" },
            agentId: { value: "agent-1" },
            userId: "u-1",
            projectId: "p-1",
            createdAt: "2026-04-30T10:00:00Z",
          },
        }),
      },
    ]);
    const life = new LifeClient({
      baseUrl: "https://gw.test",
      getAuthToken: async () => "tok-abc",
      fetch: gw.fetch,
    });

    const session = await life.agent.createSession({
      userId: "u-1",
      projectId: "p-1",
      label: "test",
    });

    expect(session.sid.value).toBe("sid-fake-1");
    expect(session.userId).toBe("u-1");
    expect(gw.recordedRequests).toHaveLength(1);
    expectAuthHeader(gw.recordedRequests[0]!.headers, "tok-abc");
  });

  it("describeSession is a unary RPC", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/DescribeSession",
        handler: async ({ body }) => {
          expect((body as { sid: { value: string } }).sid.value).toBe("sid-3");
          return { unary: { sid: { value: "sid-3" }, userId: "u-x" } };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const s = await life.agent.describeSession({ sid: { value: "sid-3" } });
    expect(s.userId).toBe("u-x");
  });

  it("sendMessage yields server-streamed events and ends on end-frame", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/SendMessage",
        handler: async () => ({
          stream: [
            {
              flag: 0,
              body: {
                record: { sessionId: { value: "sid-1" }, sequence: "1", kind: "token", payload: "" },
                kind: "AGENT_EVENT_KIND_TOKEN",
              },
            },
            {
              flag: 0,
              body: {
                record: { sessionId: { value: "sid-1" }, sequence: "2", kind: "finish", payload: "" },
                kind: "AGENT_EVENT_KIND_FINISH",
              },
            },
            { flag: 2, body: {} }, // end-stream
          ],
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });

    const events: Array<{ kind: string }> = [];
    for await (const e of life.agent.sendMessage({
      sid: { value: "sid-1" },
      content: "hi",
    })) {
      events.push(e);
    }

    expect(events).toHaveLength(2);
    expect(events[0]?.kind).toBe("AGENT_EVENT_KIND_TOKEN");
    expect(events[1]?.kind).toBe("AGENT_EVENT_KIND_FINISH");
  });

  it("propagates Unauthenticated as AuthError", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/CreateSession",
        handler: async () => ({
          status: 401,
          unary: { code: "unauthenticated", message: "bad token" },
        }),
      },
    ]);
    const life = new LifeClient({
      baseUrl: "https://gw.test",
      getAuthToken: async () => "tok-bad",
      fetch: gw.fetch,
    });

    await expect(life.agent.createSession({ userId: "u" })).rejects.toBeInstanceOf(AuthError);
  });

  it("approveDispatch + cancelDispatch are unary noops", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/ApproveDispatch",
        handler: async () => ({ unary: {} }),
      },
      {
        path: "/life.v1.Agent/CancelDispatch",
        handler: async () => ({ unary: {} }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    await life.agent.approveDispatch({ sid: { value: "sid" }, dispatchId: "d1" });
    await life.agent.cancelDispatch({ sid: { value: "sid" }, dispatchId: "d2" });
    expect(gw.recordedRequests).toHaveLength(2);
  });

  it("listSkills returns a CatalogEntry array", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Agent/ListSkills",
        handler: async () => ({
          unary: {
            items: [
              { id: "skill-1", version: "1.0.0", manifest: "" },
              { id: "skill-2", version: "2.0.0", manifest: "" },
            ],
          },
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const cat = await life.agent.listSkills({ projectId: "p" });
    expect(cat.items).toHaveLength(2);
    expect(cat.items[0]?.id).toBe("skill-1");
  });
});
