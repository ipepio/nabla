// Curated catalogue of MCP server templates (F0 of
// `odd/tasks/tool-drawer-and-mcp.md`, "Catálogo de MCPs").
//
// An entry is DATA, never code: a label, the transport, and the argv (`command` +
// `args`) or the URL pi would be told to use. Picking one only fills the add form;
// the human still reads "what runs" and presses save, and `mcp_add` writes pi's
// own `mcp.json`. Nothing here downloads, installs or upgrades anything, and no
// entry can run on its own.
//
// Why a curated list and not an installer: taking an arbitrary package name or URL
// from the web is a supply-chain surface, and it is the same reason `pi install`
// is outside the CLI bridge allowlist. A template earns its place here only when
// its command (or URL) is one this project could check — the official reference
// servers, the Chrome DevTools server this feature was scoped around, and the one
// OAuth endpoint pi's own MCP documentation names.
//
// `requiredKeys` lists the environment variable NAMES a template needs. The form
// turns each one into an empty row: the value is typed by the human, goes into
// pi's plain-text `mcp.json`, and is never sent back across the seam. An empty
// list means the template needs no credential.
//
// The `note` is the one line the pane shows for the template itself: what it is,
// in words that stay true no matter what the person types into the form.

import type { McpTransport } from "@/contracts/types";

export interface McpCatalogueEntry {
  /** Stable id, also the server name the form prefills (letters, digits, `-`). */
  id: string;
  /** Product name. Brand names are literal and never translated. */
  label: string;
  /** What the server offers, one sentence. */
  description: string;
  transport: McpTransport;
  /** stdio only: the executable. */
  command?: string;
  /** stdio only: its arguments, in order. */
  args?: string[];
  /** http only: the streamable HTTP endpoint. */
  url?: string;
  /** Environment variable names the template needs, prefilled as empty rows. */
  requiredKeys: string[];
  /**
   * What choosing this template will actually run or connect to, for the human to
   * read BEFORE saving. One line, no marketing.
   */
  note: string;
}

/**
 * The templates the add form offers, in the order it shows them: the two official
 * reference servers first (the cheapest way to prove the wiring works), then the
 * two that hold the real use cases — driving a local Chromium and reading a
 * hosted service that signs in with OAuth.
 */
export const MCP_CATALOGUE: readonly McpCatalogueEntry[] = [
  {
    id: "everything",
    label: "MCP reference server",
    description:
      "The protocol's own test server: a handful of tools, resources and prompts meant for checking that a client works.",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-everything"],
    requiredKeys: [],
    note: "Runs npx -y @modelcontextprotocol/server-everything: the reference server from the MCP project, used to test a client end to end.",
  },
  {
    id: "filesystem",
    label: "Filesystem",
    description:
      "Read, write, search and list files under the folders you name as arguments — and nowhere else.",
    transport: "stdio",
    command: "npx",
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/absolute/path"],
    requiredKeys: [],
    note: "Runs npx -y @modelcontextprotocol/server-filesystem <folder>: the official filesystem server, scoped to the absolute folders in its arguments.",
  },
  {
    id: "chrome-devtools",
    label: "Chrome DevTools",
    description:
      "Drives a real Chromium over CDP: navigate, click, type, take screenshots and read the page — the browser capability of this feature without writing an extension.",
    transport: "stdio",
    command: "npx",
    args: ["-y", "chrome-devtools-mcp@latest"],
    requiredKeys: [],
    note: "Runs npx -y chrome-devtools-mcp@latest: the Chrome DevTools MCP server, which controls a local Chromium over the DevTools protocol.",
  },
  {
    id: "sentry",
    label: "Sentry",
    description:
      "Issues, events and traces of a Sentry organization, exposed as tools.",
    transport: "http",
    url: "https://mcp.sentry.dev/mcp",
    requiredKeys: [],
    note: "Connects to https://mcp.sentry.dev/mcp: a hosted server that needs no token in the file. It reports `needs sign-in` until you sign in from this pane, and pi keeps the tokens in ~/.pi/agent/mcp-auth.json.",
  },
] as const;

/** One template by id, `undefined` when the id is not in the catalogue. */
export function findCatalogueEntry(
  id: string,
): McpCatalogueEntry | undefined {
  return MCP_CATALOGUE.find((entry) => entry.id === id);
}
