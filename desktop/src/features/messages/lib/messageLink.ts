/**
 * `buzz://message` link encoding for "Copy link" / deep-link-to-message.
 *
 * Format: `buzz://message?channel=<uuid>&id=<eventId>[&thread=<rootId>]`
 */

// FORK-LOCAL PATCH (adrienlacombe/buzz): emit `bitcoinmarkets:`, accept both.
//
// Only `bitcoinmarkets` is registered with the OS, so a freshly copied link has
// to use it or it opens upstream Buzz — or nothing. `buzz:` stays accepted
// because those links are already sitting in message history and would otherwise
// render as dead text.
const MESSAGE_LINK_SCHEME = "bitcoinmarkets:";
const LEGACY_MESSAGE_LINK_SCHEME = "buzz:";
const MESSAGE_LINK_HOST = "message";

export type MessageLinkInput = {
  channelId: string;
  messageId: string;
  /**
   * Optional thread root event id. Present when the linked message is a
   * reply (so the caller can route into a thread / forum post view).
   *
   * Currently emitted into the URL but not consumed by the click handler
   * or deep-link listener — both route via `goChannel(channelId,
   * { messageId })` and let `useAnchoredScroll` resolve the target.
   * Reserved for future "open in thread view" routing.
   */
  threadRootId?: string | null;
};

export type ParsedMessageLink = {
  channelId: string;
  messageId: string;
  threadRootId: string | null;
};

export type MessageLinkParseResult =
  | { ok: true; value: ParsedMessageLink }
  | { ok: false; reason: string };

/**
 * Build a `buzz://message` URL for a given channel + message.
 *
 * Empty `threadRootId` is treated as "no thread" so callers can pass through
 * the result of `getThreadReference(tags).rootId` without extra null checks.
 */
export function buildMessageLink(input: MessageLinkInput): string {
  if (!input.channelId) {
    throw new Error("buildMessageLink: channelId is required");
  }
  if (!input.messageId) {
    throw new Error("buildMessageLink: messageId is required");
  }

  const params = new URLSearchParams();
  params.set("channel", input.channelId);
  params.set("id", input.messageId);
  if (input.threadRootId) {
    params.set("thread", input.threadRootId);
  }
  return `${MESSAGE_LINK_SCHEME}//${MESSAGE_LINK_HOST}?${params.toString()}`;
}

/**
 * Parse a `buzz://message?…` URL. Returns a discriminated result so callers can
 * render a fallback (e.g. a plain link) without throwing.
 */
export function parseMessageLink(url: string): MessageLinkParseResult {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { ok: false, reason: "invalid-url" };
  }

  if (
    parsed.protocol !== MESSAGE_LINK_SCHEME &&
    parsed.protocol !== LEGACY_MESSAGE_LINK_SCHEME
  ) {
    return { ok: false, reason: "wrong-scheme" };
  }
  // `new URL("buzz://message?…")` puts "message" in `hostname`.
  if (parsed.hostname !== MESSAGE_LINK_HOST) {
    return { ok: false, reason: "wrong-host" };
  }

  const channelId = parsed.searchParams.get("channel");
  const messageId = parsed.searchParams.get("id");
  if (!channelId) {
    return { ok: false, reason: "missing-channel" };
  }
  if (!messageId) {
    return { ok: false, reason: "missing-id" };
  }

  return {
    ok: true,
    value: {
      channelId,
      messageId,
      threadRootId: parsed.searchParams.get("thread") ?? null,
    },
  };
}

/**
 * Convenience: returns true if the given href is a supported message link.
 * Cheap pre-check used by the markdown renderer before parsing.
 */
export function isMessageLink(href: string | undefined | null): boolean {
  if (!href) return false;
  // Both schemes, matching parseMessageLink — this is the markdown renderer's
  // pre-check, so a miss here renders an existing link as inert text.
  const prefixes = [MESSAGE_LINK_SCHEME, LEGACY_MESSAGE_LINK_SCHEME];
  return prefixes.some(
    (scheme) =>
      href.startsWith(`${scheme}//${MESSAGE_LINK_HOST}?`) ||
      href === `${scheme}//${MESSAGE_LINK_HOST}`,
  );
}

type MessageLinkRenderInput = {
  href: string;
  label: string;
};

export type MessageLinkRenderTarget =
  | { kind: "pill"; link: ParsedMessageLink }
  | { kind: "label"; link: ParsedMessageLink }
  | { kind: "none" };

/**
 * Centralizes how markdown-rendered anchors map to message-link UI. Both
 * CommonMark autolinks (`<buzz://message?...>`) and explicitly labeled links
 * arrive as anchors; autolinks have label === href and should render as pills,
 * while intentionally labeled links keep their label.
 */
export function resolveMessageLinkRenderTarget({
  href,
  label,
}: MessageLinkRenderInput): MessageLinkRenderTarget {
  if (!isMessageLink(href)) return { kind: "none" };

  const parsed = parseMessageLink(href);
  if (!parsed.ok) return { kind: "none" };

  return {
    kind: label === href ? "pill" : "label",
    link: parsed.value,
  };
}
