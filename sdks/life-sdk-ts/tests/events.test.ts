/**
 * `life.v1.Events` service unit tests.
 */

import { describe, expect, it } from "vitest";
import { LifeClient } from "../src/index.js";
import { FakeGateway } from "./_helpers.js";

describe("EventsClient", () => {
  it("read streams EventRecord frames", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Events/Read",
        handler: async () => ({
          stream: [
            { flag: 0, body: { sessionId: { value: "sid-1" }, sequence: "1", kind: "agent_token", payload: "" } },
            { flag: 0, body: { sessionId: { value: "sid-1" }, sequence: "2", kind: "agent_token", payload: "" } },
            { flag: 2, body: {} },
          ],
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const records: Array<{ sequence: string }> = [];
    for await (const r of life.events.read({
      sessionId: { value: "sid-1" },
      fromSequence: 0n,
    })) {
      records.push(r as unknown as { sequence: string });
    }
    expect(records).toHaveLength(2);
  });

  it("subscribe streams events with kind filter forwarded to server", async () => {
    let capturedFilter: string[] | undefined;
    const gw = new FakeGateway([
      {
        path: "/life.v1.Events/Subscribe",
        handler: async ({ body }) => {
          capturedFilter = (body as { kinds?: string[] }).kinds;
          return {
            stream: [
              { flag: 0, body: { sessionId: { value: "sid" }, sequence: "5", kind: "tool_result", payload: "" } },
              { flag: 2, body: {} },
            ],
          };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });

    const out: unknown[] = [];
    for await (const e of life.events.subscribe({
      sessionId: { value: "sid" },
      kinds: ["tool_result"],
    })) {
      out.push(e);
    }
    expect(out).toHaveLength(1);
    expect(capturedFilter).toEqual(["tool_result"]);
  });

  it("getBlob is a unary RPC returning the Blob shape", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Events/GetBlob",
        handler: async ({ body }) => {
          expect((body as { sha256: string }).sha256).toBe("abc123");
          return {
            unary: {
              data: "aGVsbG8=", // "hello" base64
              contentType: "text/plain",
            },
          };
        },
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const blob = await life.events.getBlob({ namespace: "ns", sha256: "abc123" });
    expect(blob.contentType).toBe("text/plain");
  });

  it("getBlobBytes round-trips base64 to Uint8Array", async () => {
    const gw = new FakeGateway([
      {
        path: "/life.v1.Events/GetBlob",
        handler: async () => ({
          unary: { data: "aGVsbG8=", contentType: "text/plain" },
        }),
      },
    ]);
    const life = new LifeClient({ baseUrl: "https://gw.test", fetch: gw.fetch });
    const bytes = await life.events.getBlobBytes({ namespace: "ns", sha256: "x" });
    expect(new TextDecoder().decode(bytes)).toBe("hello");
  });
});
