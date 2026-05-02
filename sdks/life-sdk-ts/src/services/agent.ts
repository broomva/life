/**
 * `life.v1.Agent` service client.
 *
 * @see proto/life/v1/agent.proto
 */

import type { Transport, TransportCallOptions } from "../transport.js";
import type {
  AgentEvent,
  ApprovalReq,
  CreateSessionReq,
  DispatchRef,
  Empty,
  ListModelsReq,
  ListSkillsReq,
  ListToolsReq,
  ModelCatalog,
  SendMessageReq,
  Session,
  SessionRef,
  SkillCatalog,
  SpawnChildReq,
  SpawnChildResp,
  ToolCatalog,
} from "../proto/agent.js";

/**
 * Service identifier for routing on lifegw. Spec C₂ §3.1.
 */
const SERVICE = "life.v1.Agent";

export class AgentClient {
  constructor(private readonly transport: Transport) {}

  /**
   * Create a new session.
   *
   * Wraps `Agent.CreateSession` (unary).
   */
  createSession(req: CreateSessionReq, opts?: TransportCallOptions): Promise<Session> {
    return this.transport.unary<CreateSessionReq, Session>(
      SERVICE,
      "CreateSession",
      req,
      opts,
    );
  }

  /**
   * Describe an existing session — read-only metadata fetch.
   */
  describeSession(req: SessionRef, opts?: TransportCallOptions): Promise<Session> {
    return this.transport.unary<SessionRef, Session>(
      SERVICE,
      "DescribeSession",
      req,
      opts,
    );
  }

  /**
   * Close a session.
   *
   * Closing twice is idempotent (the second call resolves with
   * `Empty`).
   */
  closeSession(req: SessionRef, opts?: TransportCallOptions): Promise<Empty> {
    return this.transport.unary<SessionRef, Empty>(
      SERVICE,
      "CloseSession",
      req,
      opts,
    );
  }

  /**
   * Send a chat message and receive a server-streaming `AgentEvent`
   * tail.
   *
   * Yields events until the upstream stream finishes (FINISH or
   * ERROR kind) or the call is aborted.
   *
   * For long-lived sessions where the client may reconnect, prefer
   * the WebSocket transport via {@link AgentClient.streamSessionWs}.
   */
  sendMessage(
    req: SendMessageReq,
    opts?: TransportCallOptions,
  ): AsyncIterable<AgentEvent> {
    return this.transport.serverStream<SendMessageReq, AgentEvent>(
      SERVICE,
      "SendMessage",
      req,
      opts,
    );
  }

  /**
   * Tail an existing session's events over server-streaming gRPC.
   *
   * Resume cursor: pass `req.fromSequence` to replay from `N+1`. For
   * browser hosts that need true reconnect-on-drop semantics, prefer
   * the WebSocket helper {@link AgentClient.streamSessionWs}.
   */
  streamSession(
    req: SessionRef,
    opts?: TransportCallOptions,
  ): AsyncIterable<AgentEvent> {
    return this.transport.serverStream<SessionRef, AgentEvent>(
      SERVICE,
      "StreamSession",
      req,
      opts,
    );
  }

  /**
   * Approve a tool dispatch waiting on user consent.
   */
  approveDispatch(req: ApprovalReq, opts?: TransportCallOptions): Promise<Empty> {
    return this.transport.unary<ApprovalReq, Empty>(
      SERVICE,
      "ApproveDispatch",
      req,
      opts,
    );
  }

  /**
   * Cancel a pending dispatch.
   */
  cancelDispatch(req: DispatchRef, opts?: TransportCallOptions): Promise<Empty> {
    return this.transport.unary<DispatchRef, Empty>(
      SERVICE,
      "CancelDispatch",
      req,
      opts,
    );
  }

  /** List skill catalog entries available to a project. */
  listSkills(req: ListSkillsReq, opts?: TransportCallOptions): Promise<SkillCatalog> {
    return this.transport.unary<ListSkillsReq, SkillCatalog>(
      SERVICE,
      "ListSkills",
      req,
      opts,
    );
  }

  /** List model catalog entries available to a project. */
  listModels(req: ListModelsReq, opts?: TransportCallOptions): Promise<ModelCatalog> {
    return this.transport.unary<ListModelsReq, ModelCatalog>(
      SERVICE,
      "ListModels",
      req,
      opts,
    );
  }

  /** List tool catalog entries available to a project. */
  listTools(req: ListToolsReq, opts?: TransportCallOptions): Promise<ToolCatalog> {
    return this.transport.unary<ListToolsReq, ToolCatalog>(
      SERVICE,
      "ListTools",
      req,
      opts,
    );
  }

  /**
   * Spawn a child session. M5 ships a stub server-side — the client
   * still surfaces the call so consumers can detect when the
   * platform graduates the feature.
   */
  spawnChild(
    req: SpawnChildReq,
    opts?: TransportCallOptions,
  ): Promise<SpawnChildResp> {
    return this.transport.unary<SpawnChildReq, SpawnChildResp>(
      SERVICE,
      "SpawnChild",
      req,
      opts,
    );
  }
}
