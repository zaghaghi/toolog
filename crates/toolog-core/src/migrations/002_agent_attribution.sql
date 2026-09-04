-- Migration 002 — subagent attribution and session slug.
--
-- Phase 2's investigation of the real corpus corrected an assumption in the
-- plan. Three distinct things were being conflated:
--
--   agentId           A subagent *instance*. Present on 100% of sidechain
--                     records (658/658) and 0% of main-thread records
--                     (0/5944), so it is the reliable discriminator.
--   attributionAgent  The subagent *type* ("Explore", "general-purpose").
--                     Only on ~57% of sidechain records, so it is best-effort
--                     and backfilled per agentId.
--   agent-name        A *session* label such as "host-password-reset-flow" —
--                     a worktree/agent-session name, unrelated to subagents.
--                     A session carrying one may have no sidechain records at
--                     all, which is how the conflation was caught.
--
-- The authoritative link is the spawning `Agent` tool call: its result carries
-- the same `agentId` its sidechain records do.

ALTER TABLE tool_call ADD COLUMN agent_id TEXT;

CREATE INDEX tool_call_agent_id ON tool_call (agent_id);

-- The session's own slug, e.g. "plan-for-admin-users-velvety-ember".
ALTER TABLE session ADD COLUMN slug TEXT;
