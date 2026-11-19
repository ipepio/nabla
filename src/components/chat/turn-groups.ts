// Grouping a session's transcript by the human turn (G1/G2 of
// `chat-turn-groups.md`, spec S1).
//
// One turn is what happens between one human message and the next. pi answers a
// turn with many steps — one assistant message each, with its own reasoning and
// its own tool calls — and rendering a box per step is what made a 13-step turn
// read as 26 boxes. This module turns the flat transcript plus the session's
// chips into the shape the panel wants: one group per turn, its steps in order,
// and the turn's final answer singled out (S2).
//
// Two sources, one rule. A live turn carries attribution the store stamped
// (`ChatMessage.turnId` on each step, `ToolEntry.turnId`/`stepId` on each chip),
// while a transcript recovered from pi's JSONL file carries no ids at all and
// nothing about it is persisted. Both are grouped by message order — a `user`
// line opens a turn and everything after it belongs to that turn — so a restored
// conversation and a live one are read the same way, and the stamped ids only
// confirm where a step belongs (S5: no new persisted state).
import type { ChatMessage, SettledQuestion, ToolEntry } from "@/lib/session-store";

/** One step of a human turn: what pi did once before its next step. */
export interface TurnStep {
  /** The step's key, and the id its reasoning block is remembered under. */
  id: string;
  /**
   * The assistant message the step streamed into, or `null` for a step pi only
   * ran tools in (it flushed no text at all) and for a recovered `tool` note.
   */
  message: ChatMessage | null;
  /** The chips of the tools this step called, oldest first. */
  tools: ToolEntry[];
  /** Recovered `tool` transcript lines that belong to this step. */
  notes: ChatMessage[];
}

/** One human turn and everything it produced. */
export interface TurnGroup {
  /** The turn's key: the id of the human message that opened it. */
  id: string;
  /** The human message that opened it; `null` for a turn with no opening line. */
  prompt: ChatMessage | null;
  /** A settled question that opens the turn, rendered as its card instead. */
  question: SettledQuestion | null;
  /** The turn's finished steps, oldest first. */
  steps: TurnStep[];
  /**
   * The turn's final answer (S2): the last step that wrote text. `null` while the
   * turn is still running — its answer is still streaming and the panel renders
   * that instead — and for a turn that only produced work and no words.
   */
  answer: ChatMessage | null;
  /**
   * Every chip of the turn, in the order they were reported. The collapsed row
   * reads its count, its per-name breakdown and its failure count from here, so a
   * failure is never hidden behind the disclosure (S4).
   */
  tools: ToolEntry[];
  /** The turn is the one running right now and has not been folded yet (S3). */
  live: boolean;
  /**
   * Chips of the step in progress, which no committed message owns yet. Only a
   * live turn has them: once it ends they become a step inside the disclosure.
   */
  liveTools: ToolEntry[];
}

/** A group that starts with no human line above its first step. */
const HEAD_GROUP_PREFIX = "turn-head:";

/** Key of the step that holds chips whose step message never landed. */
function chipStepId(groupId: string): string {
  return `${groupId}:chips`;
}

/**
 * Turns a session's transcript and chips into one group per human turn.
 *
 * `liveTurnId` is the turn the store is answering right now (`SessionBuffer.turnId`
 * while the session is busy); that group is the only one that renders its current
 * step outside the disclosure. Everything else is settled, answer included.
 */
export function deriveTurnGroups(
  history: ChatMessage[],
  chips: ToolEntry[],
  liveTurnId: string | null,
): TurnGroup[] {
  const groups: TurnGroup[] = [];
  const groupsById = new Map<string, TurnGroup>();
  const stepsById = new Map<string, TurnStep>();

  function openGroup(id: string, opening: ChatMessage | null): TurnGroup {
    const group: TurnGroup = {
      id,
      prompt: opening !== null && opening.question === undefined ? opening : null,
      question: opening?.question ?? null,
      steps: [],
      answer: null,
      tools: [],
      live: id === liveTurnId,
      liveTools: [],
    };
    groups.push(group);
    groupsById.set(id, group);
    return group;
  }

  function openStep(group: TurnGroup, message: ChatMessage | null): TurnStep {
    const step: TurnStep = {
      id: message?.id ?? chipStepId(group.id),
      message,
      tools: [],
      notes: [],
    };
    group.steps.push(step);
    stepsById.set(step.id, step);
    return step;
  }

  let current: TurnGroup | null = null;
  for (const message of history) {
    if (message.role === "user") {
      // S1: a human line opens a turn. A question card is a human line too — the
      // answer that continues the exchange — so the work that follows it reads
      // under the card instead of disappearing behind the previous fold.
      current = openGroup(message.id, message);
      continue;
    }
    // A step the store attributed to a known turn goes there even when the
    // order says otherwise (a transcript seeded under a live turn).
    const attributed =
      message.turnId === undefined ? undefined : groupsById.get(message.turnId);
    if (attributed !== undefined) current = attributed;
    // A step above any human line (a recovered transcript that starts mid-turn)
    // still needs a turn to live in; it gets an anonymous one.
    else if (current === null) current = openGroup(`${HEAD_GROUP_PREFIX}${message.id}`, null);
    if (message.role === "assistant") {
      openStep(current, message);
      continue;
    }
    // A recovered `tool` line belongs to the step it followed; with no step yet
    // it opens one of its own, so nothing is dropped from the conversation.
    const last = current.steps[current.steps.length - 1];
    if (last === undefined) openStep(current, null).notes.push(message);
    else last.notes.push(message);
  }

  for (const chip of chips) {
    const step = stepsById.get(chip.stepId);
    if (step !== undefined) {
      step.tools.push(chip);
      continue;
    }
    // No step message owns this chip: the step it ran in has not flushed yet, or
    // never wrote a word. It goes to the turn it names, or to the turn on screen.
    const owner = groupsById.get(chip.turnId) ?? groups[groups.length - 1];
    if (owner === undefined) continue;
    owner.liveTools.push(chip);
  }

  for (const group of groups) {
    // A settled turn keeps every chip inside its disclosure: the ones no step
    // message claimed become a chip-only step, so the fold is still complete.
    if (!group.live && group.liveTools.length > 0) {
      group.steps.push({
        id: chipStepId(group.id),
        message: null,
        tools: group.liveTools,
        notes: [],
      });
      group.liveTools = [];
    }
    group.tools = [
      ...group.steps.flatMap((step) => step.tools),
      ...group.liveTools,
    ];
    if (group.live) continue;
    // S2: the answer is the last thing the turn said in words. A live turn has
    // none yet — its answer is still streaming and the panel paints that.
    for (let index = group.steps.length - 1; index >= 0; index -= 1) {
      const message = group.steps[index]?.message ?? null;
      if (message !== null && message.text !== "") {
        group.answer = message;
        break;
      }
    }
  }

  return groups;
}
