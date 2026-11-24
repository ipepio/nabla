<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import {
  BoldIcon,
  BookOpenIcon,
  CheckSquareIcon,
  ChevronDownIcon,
  CodeIcon,
  CopyIcon,
  FileDownIcon,
  FilePlus2Icon,
  FileTextIcon,
  FolderOpenIcon,
  Heading1Icon,
  Heading2Icon,
  Heading3Icon,
  Heading4Icon,
  ImagePlusIcon,
  ItalicIcon,
  LinkIcon,
  ListIcon,
  ListOrderedIcon,
  MessageSquarePlusIcon,
  MinusIcon,
  PlusIcon,
  PrinterIcon,
  QuoteIcon,
  SaveIcon,
  SparklesIcon,
  TableIcon,
  TextSelectIcon,
  Trash2Icon,
  TypeIcon,
  WandSparklesIcon,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  commandErrorMessage,
  docCreateTemp,
  docRead,
  docWrite,
  exportDocxFile,
  pickImageFiles,
  pickMarkdownFile,
  pickSaveDocxFile,
  pickSaveMarkdownFile,
  readImageAttachment,
} from "@/contracts/commands";
import type { WorkspaceInfo } from "@/contracts/types";
import { t, type MessageKey } from "@/lib/i18n";
import {
  requestAgentWithDocumentSelection,
  type DocumentAgentAction,
} from "@/lib/agent-selection";
import {
  createTextBlock,
  detectMarkdownShortcut,
  isTextBlock,
  nextBlockId,
  parseMarkdownToBlocks,
  serializeBlocksToMarkdown,
  type DocBlock,
  type TextBlockKind,
} from "./document-blocks";

const props = defineProps<{
  path: string;
  workspace: WorkspaceInfo | null;
}>();

const currentPath = ref<string>(props.path || "");
const isTempDoc = ref(false);
const blocks = ref<DocBlock[]>([]);
const savedSnapshot = ref<string>("");
const lastDiskContent = ref<string>("");
const externalUpdatePending = ref(false);
const focusedBlockId = ref<string | null>(null);
const loading = ref(false);
const saving = ref(false);
const exporting = ref(false);
const error = ref<string | null>(null);
const statusNotice = ref<string | null>(null);

// Slash command popover state (Notion-style `/` block menu)
const slashMenuBlockId = ref<string | null>(null);
const slashQuery = ref("");
const slashActiveIndex = ref(0);

// Multi-block selection state (drag across blocks, Shift+Click, Cmd/Ctrl+A)
const selectedBlockIds = ref<Set<string>>(new Set());
const dragAnchorIdx = ref<number | null>(null);
const isPointerDownInDoc = ref(false);
const isMultiBlockDragging = ref(false);

// Right-click context menu for interacting with the agent on selected text/blocks
const ctxMenuVisible = ref(false);
const ctxMenuX = ref(0);
const ctxMenuY = ref(0);
const ctxSelectedText = ref("");

const hasMultiBlockSelection = computed(() => selectedBlockIds.value.size > 0);

function clearBlockSelection(): void {
  if (selectedBlockIds.value.size > 0) {
    selectedBlockIds.value = new Set();
  }
}

function selectBlockRange(startIdx: number, endIdx: number): void {
  const from = Math.max(0, Math.min(startIdx, endIdx));
  const to = Math.min(blocks.value.length - 1, Math.max(startIdx, endIdx));
  const next = new Set<string>();
  for (let i = from; i <= to; i += 1) {
    next.add(blocks.value[i].id);
  }
  selectedBlockIds.value = next;
}

function selectAllBlocks(): void {
  if (blocks.value.length === 0) return;
  selectBlockRange(0, blocks.value.length - 1);
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.blur();
  }
}

function getSelectedBlocksMarkdown(): string {
  const selected = blocks.value.filter((b) => selectedBlockIds.value.has(b.id));
  if (selected.length === 0) return "";
  return serializeBlocksToMarkdown(selected).trim();
}

function getCurrentDocumentSelectionText(): string {
  if (selectedBlockIds.value.size > 0) {
    return getSelectedBlocksMarkdown();
  }
  if (focusedBlockId.value) {
    const el = blockInputEls.get(focusedBlockId.value);
    if (el) {
      const start = el.selectionStart ?? 0;
      const end = el.selectionEnd ?? 0;
      if (end > start) {
        return el.value.slice(start, end);
      }
    }
  }
  const active = document.activeElement;
  if (active instanceof HTMLTextAreaElement || active instanceof HTMLInputElement) {
    const start = active.selectionStart ?? 0;
    const end = active.selectionEnd ?? 0;
    if (end > start) {
      return active.value.slice(start, end);
    }
  }
  return window.getSelection()?.toString() ?? "";
}

function handleBlockMouseDown(idx: number, event: MouseEvent): void {
  if (event.button !== 0) return;
  closeContextMenu();
  if (event.shiftKey && dragAnchorIdx.value !== null) {
    event.preventDefault();
    selectBlockRange(dragAnchorIdx.value, idx);
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur();
    }
    return;
  }
  clearBlockSelection();
  dragAnchorIdx.value = idx;
  isPointerDownInDoc.value = true;
  isMultiBlockDragging.value = false;
}

function handleBlockMouseEnter(idx: number): void {
  if (!isPointerDownInDoc.value || dragAnchorIdx.value === null) return;
  if (idx !== dragAnchorIdx.value || isMultiBlockDragging.value) {
    if (!isMultiBlockDragging.value) {
      isMultiBlockDragging.value = true;
      window.getSelection()?.removeAllRanges();
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
    }
    selectBlockRange(dragAnchorIdx.value, idx);
  }
}

function handleGlobalMouseUp(): void {
  isPointerDownInDoc.value = false;
  isMultiBlockDragging.value = false;
}

function closeContextMenu(): void {
  ctxMenuVisible.value = false;
}

function handleDocumentContextMenu(event: MouseEvent): void {
  if (import.meta.env.DEV && event.shiftKey) return;
  const text = getCurrentDocumentSelectionText().trim();
  if (!text) return; // Fall through to AppContextMenu when nothing is selected

  event.preventDefault();
  event.stopPropagation();
  ctxSelectedText.value = text;
  const menuW = 248;
  const menuH = 270;
  ctxMenuX.value = Math.min(event.clientX, Math.max(8, window.innerWidth - menuW - 8));
  ctxMenuY.value = Math.min(event.clientY, Math.max(8, window.innerHeight - menuH - 8));
  ctxMenuVisible.value = true;
}

function runAgentAction(action: DocumentAgentAction): void {
  const text = ctxSelectedText.value;
  closeContextMenu();
  if (!text) return;
  requestAgentWithDocumentSelection(action, text, currentPath.value || fileName.value);
}

async function copySelectedDocumentText(): Promise<void> {
  const text = ctxSelectedText.value || getCurrentDocumentSelectionText();
  closeContextMenu();
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    showNotice(t("context.copy"));
  } catch {
    document.execCommand("copy");
  }
}

function deleteSelectedBlocks(): void {
  if (selectedBlockIds.value.size === 0) return;
  blocks.value = blocks.value.filter((b) => !selectedBlockIds.value.has(b.id));
  clearBlockSelection();
  if (blocks.value.length === 0) {
    const fresh = createTextBlock("paragraph", "");
    blocks.value.push(fresh);
    focusBlock(fresh.id, false);
  }
}

const blockInputEls = new Map<string, HTMLTextAreaElement>();
let statusTimer: number | null = null;
let autoSaveTimer: number | null = null;
let diskPollTimer: number | null = null;

function setBlockInputRef(id: string, el: unknown): void {
  if (el instanceof HTMLTextAreaElement) {
    blockInputEls.set(id, el);
    autoResizeTextarea(el);
  } else {
    blockInputEls.delete(id);
  }
}

const SUPPORTS_FIELD_SIZING =
  typeof CSS !== "undefined" && CSS.supports("field-sizing", "content");

function autoResizeTextarea(el: HTMLTextAreaElement): void {
  if (SUPPORTS_FIELD_SIZING) return;
  el.style.height = "auto";
  el.style.height = `${Math.max(el.scrollHeight + 2, 26)}px`;
}

function showNotice(msg: string): void {
  statusNotice.value = msg;
  if (statusTimer !== null) window.clearTimeout(statusTimer);
  statusTimer = window.setTimeout(() => {
    statusNotice.value = null;
    statusTimer = null;
  }, 3500);
}

function looksLikeTempPath(path: string): boolean {
  return path.replace(/\\/g, "/").includes("/tmp/docs/");
}

const serializedMarkdown = computed(() =>
  blocks.value.length > 0 ? serializeBlocksToMarkdown(blocks.value) : "",
);

const isDirty = computed(
  () => currentPath.value !== "" && serializedMarkdown.value !== savedSnapshot.value,
);

const fileName = computed(() => {
  if (!currentPath.value) return "";
  const parts = currentPath.value.split(/[\\/]/);
  return parts[parts.length - 1] || currentPath.value;
});

function parentDir(filePath: string): string {
  const idx = Math.max(filePath.lastIndexOf("/"), filePath.lastIndexOf("\\"));
  return idx >= 0 ? filePath.slice(0, idx) : "";
}

function resolveImagePath(rawSrc: string): string {
  const trimmed = rawSrc.trim();
  if (/^(https?:|data:|blob:)/i.test(trimmed)) return trimmed;
  if (trimmed.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(trimmed)) return trimmed;
  const base = currentPath.value
    ? parentDir(currentPath.value)
    : (props.workspace?.path ?? "");
  if (!base) return trimmed;
  return `${base}/${trimmed.replace(/^\.\//, "")}`;
}

// ── Block menu catalog (Notion-style Basic blocks) ───────────────────────────

interface BlockMenuItem {
  id: TextBlockKind | "table" | "image" | "divider";
  labelKey: MessageKey;
  shortcut: string;
  icon: typeof TypeIcon;
}

const BASIC_BLOCKS: readonly BlockMenuItem[] = [
  { id: "paragraph", labelKey: "tools.document.block.text", shortcut: "", icon: TypeIcon },
  { id: "h1", labelKey: "tools.document.block.h1", shortcut: "#", icon: Heading1Icon },
  { id: "h2", labelKey: "tools.document.block.h2", shortcut: "##", icon: Heading2Icon },
  { id: "h3", labelKey: "tools.document.block.h3", shortcut: "###", icon: Heading3Icon },
  { id: "h4", labelKey: "tools.document.block.h4", shortcut: "####", icon: Heading4Icon },
  { id: "bullet", labelKey: "tools.document.block.bullet", shortcut: "-", icon: ListIcon },
  { id: "numbered", labelKey: "tools.document.block.numbered", shortcut: "1.", icon: ListOrderedIcon },
  { id: "todo", labelKey: "tools.document.block.todo", shortcut: "[]", icon: CheckSquareIcon },
  { id: "quote", labelKey: "tools.document.block.quote", shortcut: ">", icon: QuoteIcon },
  { id: "code", labelKey: "tools.document.block.code", shortcut: "```", icon: CodeIcon },
  { id: "table", labelKey: "tools.document.block.table", shortcut: "|", icon: TableIcon },
  { id: "image", labelKey: "tools.document.block.image", shortcut: "!", icon: ImagePlusIcon },
  { id: "divider", labelKey: "tools.document.block.divider", shortcut: "---", icon: MinusIcon },
];

const TEXT_TURN_INTO_ITEMS = BASIC_BLOCKS.filter(
  (item) => item.id !== "table" && item.id !== "image" && item.id !== "divider",
);

const filteredSlashItems = computed(() => {
  const q = slashQuery.value.trim().toLowerCase();
  if (!q) return BASIC_BLOCKS;
  return BASIC_BLOCKS.filter(
    (item) =>
      t(item.labelKey).toLowerCase().includes(q) ||
      item.id.toLowerCase().includes(q) ||
      item.shortcut.includes(q),
  );
});

const activeBlock = computed<DocBlock | null>(() => {
  if (!focusedBlockId.value) return blocks.value[0] ?? null;
  return blocks.value.find((b) => b.id === focusedBlockId.value) ?? blocks.value[0] ?? null;
});

const activeBlockLabel = computed(() => {
  const kind = activeBlock.value?.kind ?? "paragraph";
  const match = BASIC_BLOCKS.find((item) => item.id === kind);
  return match ? t(match.labelKey) : t("tools.document.block.text");
});

function numberedIndexAt(index: number): number {
  let count = 1;
  for (let i = index - 1; i >= 0; i -= 1) {
    if (blocks.value[i].kind === "numbered") count += 1;
    else break;
  }
  return count;
}

// ── File I/O ─────────────────────────────────────────────────────────────────

async function loadDocument(path: string): Promise<void> {
  const trimmed = path.trim();
  if (!trimmed) {
    currentPath.value = "";
    isTempDoc.value = false;
    blocks.value = [];
    savedSnapshot.value = "";
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    const text = await docRead(trimmed);
    currentPath.value = trimmed;
    isTempDoc.value = looksLikeTempPath(trimmed);
    lastDiskContent.value = text;
    externalUpdatePending.value = false;
    const parsed = parseMarkdownToBlocks(text);
    blocks.value = parsed;
    savedSnapshot.value = serializeBlocksToMarkdown(parsed);
    focusedBlockId.value = parsed[0]?.id ?? null;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.path,
  (nextPath) => {
    if (nextPath && nextPath !== currentPath.value) {
      void loadDocument(nextPath);
    }
  },
  { immediate: true },
);

async function handleOpen(): Promise<void> {
  error.value = null;
  try {
    const picked = await pickMarkdownFile(
      t("tools.document.pickOpenTitle"),
      props.workspace?.path,
    );
    if (picked) await loadDocument(picked);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleNew(): Promise<void> {
  error.value = null;
  try {
    const tempPath = await docCreateTemp();
    currentPath.value = tempPath;
    isTempDoc.value = true;
    const initialBlocks: DocBlock[] = [
      createTextBlock("h1", ""),
      createTextBlock("paragraph", ""),
    ];
    blocks.value = initialBlocks;
    savedSnapshot.value = serializeBlocksToMarkdown(initialBlocks);
    focusedBlockId.value = initialBlocks[0].id;
    await nextTick();
    focusBlock(initialBlocks[0].id, true);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

async function handleSaveAs(): Promise<boolean> {
  error.value = null;
  try {
    const defaultTarget = props.workspace?.path
      ? `${props.workspace.path}/document.md`
      : "document.md";
    const picked = await pickSaveMarkdownFile(
      t("tools.document.pickSaveTitle"),
      defaultTarget,
    );
    if (!picked) return false;
    const md = serializedMarkdown.value;
    await docWrite(picked, md);
    currentPath.value = picked;
    isTempDoc.value = false;
    savedSnapshot.value = md;
    lastDiskContent.value = md;
    externalUpdatePending.value = false;
    showNotice(t("tools.document.saved"));
    return true;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
    return false;
  }
}

async function handleSave(): Promise<boolean> {
  if (!currentPath.value) {
    await handleNew();
    return Boolean(currentPath.value);
  }
  if (saving.value) return false;
  saving.value = true;
  error.value = null;
  try {
    const md = serializedMarkdown.value;
    await docWrite(currentPath.value, md);
    savedSnapshot.value = md;
    lastDiskContent.value = md;
    externalUpdatePending.value = false;
    if (!isTempDoc.value) showNotice(t("tools.document.saved"));
    return true;
  } catch (cause) {
    error.value = commandErrorMessage(cause);
    return false;
  } finally {
    saving.value = false;
  }
}

// Auto-save temporary scratch documents transparently.
watch(serializedMarkdown, (md) => {
  if (!isTempDoc.value || !currentPath.value || md === savedSnapshot.value) return;
  if (autoSaveTimer !== null) window.clearTimeout(autoSaveTimer);
  autoSaveTimer = window.setTimeout(() => {
    autoSaveTimer = null;
    if (isTempDoc.value && currentPath.value) {
      const latest = serializedMarkdown.value;
      void docWrite(currentPath.value, latest)
        .then(() => {
          savedSnapshot.value = latest;
          lastDiskContent.value = latest;
        })
        .catch(() => {});
    }
  }, 800);
});

async function handleExportDocx(): Promise<void> {
  if (!currentPath.value || exporting.value) return;
  if (isDirty.value) {
    const ok = await handleSave();
    if (!ok) return;
  }
  let outputTarget: string | null = null;
  if (isTempDoc.value) {
    const defaultOut = props.workspace?.path
      ? `${props.workspace.path}/document.docx`
      : "document.docx";
    const picked = await pickSaveDocxFile(
      t("tools.document.pickExportDocxTitle"),
      defaultOut,
    );
    if (!picked) return;
    outputTarget = picked;
  }
  exporting.value = true;
  error.value = null;
  try {
    const outPath = await exportDocxFile(currentPath.value, outputTarget);
    showNotice(t("tools.document.exportedDocx", { path: outPath }));
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  } finally {
    exporting.value = false;
  }
}

// ── Block editing & Slash menu ───────────────────────────────────────────────

function focusBlock(id: string, atEnd = true): void {
  focusedBlockId.value = id;
  void nextTick(() => {
    const el = blockInputEls.get(id);
    if (!el) return;
    el.focus();
    autoResizeTextarea(el);
    const pos = atEnd ? el.value.length : 0;
    el.setSelectionRange(pos, pos);
  });
}

function closeSlashMenu(): void {
  slashMenuBlockId.value = null;
  slashQuery.value = "";
  slashActiveIndex.value = 0;
}

function convertBlockKind(blockId: string, nextKind: TextBlockKind): void {
  const idx = blocks.value.findIndex((b) => b.id === blockId);
  if (idx === -1) return;
  const current = blocks.value[idx];
  const text = isTextBlock(current) ? current.text : "";
  const replaced = createTextBlock(nextKind, text);
  replaced.id = current.id;
  blocks.value.splice(idx, 1, replaced);
  focusBlock(replaced.id, true);
}

async function applyBlockChoice(
  blockId: string,
  choice: BlockMenuItem["id"],
  clearSlashText = false,
): Promise<void> {
  const idx = blocks.value.findIndex((b) => b.id === blockId);
  if (idx === -1) return;
  closeSlashMenu();

  if (choice === "image") {
    await insertImageAt(idx, clearSlashText);
    return;
  }
  if (choice === "table") {
    const tableBlock: DocBlock = {
      id: nextBlockId(),
      kind: "table",
      headers: ["Column 1", "Column 2"],
      rows: [["", ""]],
    };
    if (clearSlashText) {
      blocks.value.splice(idx, 1, tableBlock, createTextBlock("paragraph", ""));
    } else {
      blocks.value.splice(idx + 1, 0, tableBlock);
    }
    return;
  }
  if (choice === "divider") {
    const nextPara = createTextBlock("paragraph", "");
    if (clearSlashText) {
      blocks.value.splice(idx, 1, { id: nextBlockId(), kind: "divider" }, nextPara);
    } else {
      blocks.value.splice(idx + 1, 0, { id: nextBlockId(), kind: "divider" }, nextPara);
    }
    focusBlock(nextPara.id, false);
    return;
  }

  const current = blocks.value[idx];
  const baseText = clearSlashText ? "" : isTextBlock(current) ? current.text : "";
  const replaced = createTextBlock(choice, baseText);
  replaced.id = current.id;
  blocks.value.splice(idx, 1, replaced);
  focusBlock(replaced.id, true);
}

async function insertImageAt(index: number, replaceEmpty = false): Promise<void> {
  error.value = null;
  try {
    const picked = await pickImageFiles();
    if (!picked || picked.length === 0) return;
    const baseDir = currentPath.value ? parentDir(currentPath.value) : "";
    const newBlocks: DocBlock[] = picked.map((imgPath) => {
      const name = imgPath.split(/[\\/]/).pop() ?? "image";
      const alt = name.replace(/\.[^.]+$/, "");
      const rel =
        baseDir && imgPath.startsWith(`${baseDir}/`)
          ? imgPath.slice(baseDir.length + 1)
          : imgPath;
      return { id: nextBlockId(), kind: "image", alt, src: rel };
    });
    const trailing = createTextBlock("paragraph", "");
    if (replaceEmpty) {
      blocks.value.splice(index, 1, ...newBlocks, trailing);
    } else {
      blocks.value.splice(index + 1, 0, ...newBlocks, trailing);
    }
    focusBlock(trailing.id, false);
  } catch (cause) {
    error.value = commandErrorMessage(cause);
  }
}

function handleBlockInput(block: DocBlock, event: Event): void {
  if (!isTextBlock(block)) return;
  const el = event.target as HTMLTextAreaElement;
  autoResizeTextarea(el);
  const value = el.value;
  block.text = value;

  // Slash command detection (`/` at start of block)
  if (value.startsWith("/") && !value.includes("\n")) {
    slashMenuBlockId.value = block.id;
    slashQuery.value = value.slice(1);
    slashActiveIndex.value = 0;
    return;
  } else if (slashMenuBlockId.value === block.id) {
    closeSlashMenu();
  }

  // Notion-style live Markdown prefix shortcuts on paragraph blocks
  if (block.kind === "paragraph") {
    const shortcut = detectMarkdownShortcut(value);
    if (shortcut) {
      const idx = blocks.value.findIndex((b) => b.id === block.id);
      if (idx === -1) return;
      if (shortcut.kind === "divider") {
        const nextPara = createTextBlock("paragraph", "");
        blocks.value.splice(idx, 1, { id: nextBlockId(), kind: "divider" }, nextPara);
        focusBlock(nextPara.id, false);
        return;
      }
      const replaced = createTextBlock(shortcut.kind, shortcut.rest);
      replaced.id = block.id;
      blocks.value.splice(idx, 1, replaced);
      focusBlock(replaced.id, true);
    }
  }
}

function handleBlockKeydown(block: DocBlock, idx: number, event: KeyboardEvent): void {
  if (!isTextBlock(block)) return;

  // Handle slash menu navigation when open on this block
  if (slashMenuBlockId.value === block.id) {
    const items = filteredSlashItems.value;
    if (event.key === "ArrowDown" && items.length > 0) {
      event.preventDefault();
      slashActiveIndex.value = (slashActiveIndex.value + 1) % items.length;
      return;
    }
    if (event.key === "ArrowUp" && items.length > 0) {
      event.preventDefault();
      slashActiveIndex.value = (slashActiveIndex.value - 1 + items.length) % items.length;
      return;
    }
    if (event.key === "Enter" && items.length > 0) {
      event.preventDefault();
      const chosen = items[slashActiveIndex.value] ?? items[0];
      void applyBlockChoice(block.id, chosen.id, true);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeSlashMenu();
      return;
    }
  }

  // Inline formatting shortcuts (Cmd/Ctrl + B / I / E)
  const isMac = /mac/i.test(navigator.platform || navigator.userAgent);
  const mod = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (mod && !event.altKey) {
    const lower = event.key.toLowerCase();
    if (lower === "b") {
      event.preventDefault();
      wrapSelection("**", "**", "bold");
      return;
    }
    if (lower === "i") {
      event.preventDefault();
      wrapSelection("*", "*", "italic");
      return;
    }
    if (lower === "e") {
      event.preventDefault();
      wrapSelection("`", "`", "code");
      return;
    }
  }

  // Enter creates a new block (Shift+Enter or Code block inserts newline)
  if (event.key === "Enter" && !event.shiftKey && block.kind !== "code") {
    event.preventDefault();
    if (
      block.text === "" &&
      (block.kind === "bullet" || block.kind === "numbered" || block.kind === "todo" || block.kind === "quote")
    ) {
      convertBlockKind(block.id, "paragraph");
      return;
    }
    const nextKind: TextBlockKind =
      block.kind === "bullet" || block.kind === "numbered" || block.kind === "todo"
        ? block.kind
        : "paragraph";
    const fresh = createTextBlock(nextKind, "");
    blocks.value.splice(idx + 1, 0, fresh);
    focusBlock(fresh.id, false);
    return;
  }

  // Backspace on empty block reverts heading/list to paragraph or deletes block
  if (event.key === "Backspace" && block.text === "") {
    if (block.kind !== "paragraph") {
      event.preventDefault();
      convertBlockKind(block.id, "paragraph");
      return;
    }
    if (blocks.value.length > 1) {
      event.preventDefault();
      const prev = blocks.value[idx - 1];
      blocks.value.splice(idx, 1);
      if (prev && isTextBlock(prev)) {
        focusBlock(prev.id, true);
      }
    }
    return;
  }

  // Up/Down arrow navigation between adjacent text blocks
  const el = event.target as HTMLTextAreaElement;
  if (event.key === "ArrowUp" && el.selectionStart === 0 && idx > 0) {
    for (let i = idx - 1; i >= 0; i -= 1) {
      if (isTextBlock(blocks.value[i])) {
        event.preventDefault();
        focusBlock(blocks.value[i].id, true);
        return;
      }
    }
  }
  if (
    event.key === "ArrowDown" &&
    el.selectionStart === el.value.length &&
    idx < blocks.value.length - 1
  ) {
    for (let i = idx + 1; i < blocks.value.length; i += 1) {
      if (isTextBlock(blocks.value[i])) {
        event.preventDefault();
        focusBlock(blocks.value[i].id, false);
        return;
      }
    }
  }
}

function wrapSelection(before: string, after: string, placeholder: string): void {
  const target = activeBlock.value;
  if (!target || !isTextBlock(target)) return;
  const el = blockInputEls.get(target.id);
  if (!el) {
    target.text += `${before}${placeholder}${after}`;
    return;
  }
  const start = el.selectionStart ?? target.text.length;
  const end = el.selectionEnd ?? target.text.length;
  const selected = target.text.slice(start, end) || placeholder;
  target.text =
    target.text.slice(0, start) + before + selected + after + target.text.slice(end);
  void nextTick(() => {
    el.focus();
    autoResizeTextarea(el);
    el.setSelectionRange(start + before.length, start + before.length + selected.length);
  });
}

function removeBlock(id: string): void {
  const idx = blocks.value.findIndex((b) => b.id === id);
  if (idx === -1) return;
  blocks.value.splice(idx, 1);
  if (blocks.value.length === 0) {
    blocks.value.push(createTextBlock("paragraph", ""));
  }
}

function addTableRow(block: Extract<DocBlock, { kind: "table" }>): void {
  block.rows.push(block.headers.map(() => ""));
}

function addTableCol(block: Extract<DocBlock, { kind: "table" }>): void {
  block.headers.push(`Column ${block.headers.length + 1}`);
  for (const row of block.rows) row.push("");
}

// ── Local Image Cache ────────────────────────────────────────────────────────

const imageCache = reactive<Record<string, string>>({});
const imagePending = new Set<string>();

function ensureLocalImage(rawSrc: string): void {
  if (/^(https?:|data:|blob:)/i.test(rawSrc)) return;
  const resolved = resolveImagePath(rawSrc);
  if (imageCache[resolved] || imagePending.has(resolved)) return;
  imagePending.add(resolved);
  void readImageAttachment(resolved)
    .then((att) => {
      imageCache[resolved] = `data:${att.mimeType};base64,${att.data}`;
    })
    .catch(() => {})
    .finally(() => {
      imagePending.delete(resolved);
    });
}

function previewImageSrc(rawSrc: string): string | null {
  if (/^(https?:|data:|blob:)/i.test(rawSrc)) return rawSrc;
  return imageCache[resolveImagePath(rawSrc)] ?? null;
}

watch(
  blocks,
  (list) => {
    for (const block of list) {
      if (block.kind === "image") ensureLocalImage(block.src);
    }
  },
  { deep: true, immediate: true },
);

// ── PDF Export ───────────────────────────────────────────────────────────────

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function renderInlineHtml(text: string): string {
  let html = escapeHtml(text);
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  html = html.replace(
    /\[([^\]]+)\]\((https?:\/\/[^)]+)\)/g,
    "<a href=\"$2\">$1</a>",
  );
  return html;
}

function handleExportPdf(): void {
  const bodyHtml = blocks.value
    .map((block, idx) => {
      switch (block.kind) {
        case "h1":
          return `<h1>${renderInlineHtml(block.text)}</h1>`;
        case "h2":
          return `<h2>${renderInlineHtml(block.text)}</h2>`;
        case "h3":
          return `<h3>${renderInlineHtml(block.text)}</h3>`;
        case "h4":
          return `<h4>${renderInlineHtml(block.text)}</h4>`;
        case "paragraph":
          return block.text ? `<p>${renderInlineHtml(block.text)}</p>` : "";
        case "bullet":
          return `<p class="li">• ${renderInlineHtml(block.text)}</p>`;
        case "numbered":
          return `<p class="li">${numberedIndexAt(idx)}. ${renderInlineHtml(block.text)}</p>`;
        case "todo":
          return `<p class="li">${block.checked ? "☑" : "☐"} ${renderInlineHtml(block.text)}</p>`;
        case "quote":
          return `<blockquote>${renderInlineHtml(block.text)}</blockquote>`;
        case "code":
          return `<pre><code>${escapeHtml(block.text)}</code></pre>`;
        case "divider":
          return "<hr />";
        case "table": {
          const head = `<tr>${block.headers.map((h) => `<th>${renderInlineHtml(h)}</th>`).join("")}</tr>`;
          const rows = block.rows
            .map((r) => `<tr>${r.map((c) => `<td>${renderInlineHtml(c)}</td>`).join("")}</tr>`)
            .join("");
          return `<table><thead>${head}</thead><tbody>${rows}</tbody></table>`;
        }
        case "image": {
          const src = previewImageSrc(block.src);
          return src
            ? `<figure><img src="${src}" alt="${escapeHtml(block.alt)}" />${block.alt ? `<figcaption>${escapeHtml(block.alt)}</figcaption>` : ""}</figure>`
            : `<p><em>[Image: ${escapeHtml(block.alt || block.src)}]</em></p>`;
        }
      }
    })
    .join("\n");

  const iframe = document.createElement("iframe");
  iframe.style.position = "fixed";
  iframe.style.width = "0";
  iframe.style.height = "0";
  iframe.style.border = "0";
  document.body.appendChild(iframe);
  const doc = iframe.contentDocument;
  if (!doc || !iframe.contentWindow) {
    document.body.removeChild(iframe);
    return;
  }
  doc.open();
  doc.write(`<!doctype html><html><head><meta charset="utf-8" /><title>${escapeHtml(fileName.value || "Document")}</title>
<style>
  @page { margin: 20mm; }
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #111; line-height: 1.55; font-size: 11pt; }
  h1 { font-size: 21pt; margin: 0 0 12px; }
  h2 { font-size: 16pt; margin: 18px 0 8px; }
  h3 { font-size: 13pt; margin: 14px 0 6px; }
  h4 { font-size: 11.5pt; margin: 12px 0 4px; }
  p { margin: 0 0 8px; }
  p.li { margin: 0 0 4px; padding-left: 14px; }
  blockquote { border-left: 3px solid #bbb; margin: 10px 0; padding: 4px 12px; color: #444; }
  pre { background: #f5f5f5; padding: 10px 12px; border-radius: 6px; font-size: 9.5pt; }
  code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
  table { border-collapse: collapse; width: 100%; margin: 12px 0; font-size: 10pt; }
  th, td { border: 1px solid #ccc; padding: 6px 8px; text-align: left; }
  th { background: #f2f2f2; font-weight: 600; }
  figure { margin: 14px 0; text-align: center; }
  figure img { max-width: 100%; max-height: 460px; object-fit: contain; }
  figcaption { font-size: 9pt; color: #666; margin-top: 4px; }
  hr { border: 0; border-top: 1px solid #ddd; margin: 16px 0; }
</style></head><body>${bodyHtml}</body></html>`);
  doc.close();
  const win = iframe.contentWindow;
  const cleanup = () => {
    if (iframe.parentNode) iframe.parentNode.removeChild(iframe);
  };
  win.onafterprint = cleanup;
  setTimeout(() => {
    win.focus();
    win.print();
    setTimeout(cleanup, 1500);
  }, 150);
}

function blockTextareaClass(kind: TextBlockKind): string {
  switch (kind) {
    case "h1":
      return "text-2xl font-bold tracking-tight text-foreground leading-normal py-0.5";
    case "h2":
      return "text-lg font-semibold tracking-tight text-foreground leading-normal pt-2 pb-0.5";
    case "h3":
      return "text-base font-semibold text-foreground leading-normal pt-1 pb-0.5";
    case "h4":
      return "text-sm font-semibold text-foreground/95 leading-normal pt-1 pb-0.5";
    case "quote":
      return "text-sm italic text-muted-foreground leading-relaxed";
    case "code":
      return "font-mono text-xs text-foreground leading-relaxed";
    default:
      return "text-sm text-foreground/95 leading-relaxed";
  }
}

function blockPlaceholder(kind: TextBlockKind, idx: number): string {
  if (kind === "h1") return t("tools.document.block.h1");
  if (kind === "h2") return t("tools.document.block.h2");
  if (kind === "h3") return t("tools.document.block.h3");
  if (kind === "h4") return t("tools.document.block.h4");
  if (kind === "code") return "// code…";
  if (idx === 0 || focusedBlockId.value) return t("tools.document.slashHint");
  return "";
}

function onKeydown(event: KeyboardEvent): void {
  const isMac = /mac/i.test(navigator.platform || navigator.userAgent);
  const mod = isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (mod && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void handleSave();
    return;
  }
  if (event.key === "Escape") {
    if (ctxMenuVisible.value) {
      closeContextMenu();
      return;
    }
    if (selectedBlockIds.value.size > 0) {
      clearBlockSelection();
      return;
    }
  }
  if (selectedBlockIds.value.size > 0) {
    if (mod && event.key.toLowerCase() === "c") {
      event.preventDefault();
      void copySelectedDocumentText();
      return;
    }
    if (mod && event.key.toLowerCase() === "a") {
      event.preventDefault();
      selectAllBlocks();
      return;
    }
    if (event.key === "Backspace" || event.key === "Delete") {
      event.preventDefault();
      deleteSelectedBlocks();
      return;
    }
  } else if (mod && event.key.toLowerCase() === "a" && focusedBlockId.value) {
    // Pressing Cmd/Ctrl+A when the whole current block is already selected expands
    // the selection to the entire document (Notion-style two-stage Select All).
    const el = blockInputEls.get(focusedBlockId.value);
    if (
      el &&
      document.activeElement === el &&
      el.selectionStart === 0 &&
      el.selectionEnd === el.value.length
    ) {
      event.preventDefault();
      selectAllBlocks();
    }
  }
}

async function syncFromDiskIfChanged(): Promise<void> {
  const path = currentPath.value;
  if (!path || loading.value || saving.value) return;
  try {
    const diskText = await docRead(path);
    if (diskText === lastDiskContent.value) return;
    if (isDirty.value) {
      externalUpdatePending.value = true;
      return;
    }
    lastDiskContent.value = diskText;
    externalUpdatePending.value = false;
    const parsed = parseMarkdownToBlocks(diskText);
    blocks.value = parsed;
    savedSnapshot.value = serializeBlocksToMarkdown(parsed);
  } catch {
    // Ignore transient read errors while an external tool is mid-write.
  }
}

function handleWindowFocus(): void {
  void syncFromDiskIfChanged();
}

onMounted(() => {
  window.addEventListener("keydown", onKeydown);
  window.addEventListener("focus", handleWindowFocus);
  window.addEventListener("mouseup", handleGlobalMouseUp);
  window.addEventListener("pointerdown", closeContextMenu);
  diskPollTimer = window.setInterval(() => {
    void syncFromDiskIfChanged();
  }, 1500);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKeydown);
  window.removeEventListener("focus", handleWindowFocus);
  window.removeEventListener("mouseup", handleGlobalMouseUp);
  window.removeEventListener("pointerdown", closeContextMenu);
  if (statusTimer !== null) window.clearTimeout(statusTimer);
  if (autoSaveTimer !== null) window.clearTimeout(autoSaveTimer);
  if (diskPollTimer !== null) window.clearInterval(diskPollTimer);
});
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col overflow-hidden bg-background">
    <!-- Compact single-row Notion-style toolbar with grouped dropdown menus -->
    <div
      v-if="currentPath"
      class="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border bg-card/60 px-3 py-1.5"
    >
      <!-- File menu dropdown -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button
            variant="ghost"
            size="xs"
            class="h-7 max-w-[180px] gap-1.5 px-2 text-xs font-medium text-foreground"
            :title="currentPath"
          >
            <FileTextIcon class="size-3.5 shrink-0 text-primary" />
            <span class="truncate">{{ fileName }}</span>
            <span
              v-if="isTempDoc"
              class="shrink-0 rounded bg-secondary px-1 py-0.5 text-[9px] text-muted-foreground"
            >
              tmp
            </span>
            <span
              v-else-if="isDirty"
              class="size-1.5 shrink-0 rounded-full bg-warning"
            />
            <ChevronDownIcon class="size-3 shrink-0 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-48">
          <DropdownMenuItem class="gap-2 text-xs" @select="handleOpen">
            <FolderOpenIcon class="size-3.5" />
            {{ t("tools.document.open") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 text-xs" @select="handleNew">
            <FilePlus2Icon class="size-3.5" />
            {{ t("tools.document.new") }}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            v-if="!isTempDoc"
            class="gap-2 text-xs"
            :disabled="saving || !isDirty"
            @select="handleSave"
          >
            <SaveIcon class="size-3.5" />
            {{ t("tools.document.save") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 text-xs" @select="handleSaveAs">
            <SaveIcon class="size-3.5" />
            {{ t("tools.document.saveAs") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <span class="h-4 w-px bg-border" aria-hidden="true" />

      <!-- Block type ("Basic blocks") dropdown -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="ghost" size="xs" class="h-7 gap-1 px-2 text-xs text-foreground">
            <span>{{ activeBlockLabel }}</span>
            <ChevronDownIcon class="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-56">
          <DropdownMenuLabel class="text-[11px] text-muted-foreground">
            {{ t("tools.document.basicBlocks") }}
          </DropdownMenuLabel>
          <DropdownMenuItem
            v-for="item in TEXT_TURN_INTO_ITEMS"
            :key="item.id"
            class="flex items-center justify-between gap-2 text-xs"
            @select="activeBlock && applyBlockChoice(activeBlock.id, item.id, false)"
          >
            <span class="flex items-center gap-2">
              <component :is="item.icon" class="size-3.5 text-muted-foreground" />
              <span>{{ t(item.labelKey) }}</span>
            </span>
            <span v-if="item.shortcut" class="font-mono text-[10px] text-muted-foreground">
              {{ item.shortcut }}
            </span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <!-- Compact inline formatting icons -->
      <div class="flex items-center gap-0.5">
        <Button variant="ghost" size="icon-xs" title="Bold (⌘B)" @click="wrapSelection('**', '**', 'bold')">
          <BoldIcon class="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon-xs" title="Italic (⌘I)" @click="wrapSelection('*', '*', 'italic')">
          <ItalicIcon class="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon-xs" title="Inline code (⌘E)" @click="wrapSelection('`', '`', 'code')">
          <CodeIcon class="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon-xs" title="Link" @click="wrapSelection('[', '](https://)', 'link')">
          <LinkIcon class="size-3.5" />
        </Button>
      </div>

      <!-- Insert dropdown (Image, Table, Code, Divider) -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="ghost" size="xs" class="h-7 gap-1 px-2 text-xs text-primary">
            <PlusIcon class="size-3.5" />
            <span>{{ t("tools.document.insert") }}</span>
            <ChevronDownIcon class="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-48">
          <DropdownMenuItem
            class="gap-2 text-xs"
            @select="activeBlock && applyBlockChoice(activeBlock.id, 'image', false)"
          >
            <ImagePlusIcon class="size-3.5 text-primary" />
            {{ t("tools.document.insertImage") }}
          </DropdownMenuItem>
          <DropdownMenuItem
            class="gap-2 text-xs"
            @select="activeBlock && applyBlockChoice(activeBlock.id, 'table', false)"
          >
            <TableIcon class="size-3.5" />
            {{ t("tools.document.block.table") }}
          </DropdownMenuItem>
          <DropdownMenuItem
            class="gap-2 text-xs"
            @select="activeBlock && applyBlockChoice(activeBlock.id, 'code', false)"
          >
            <CodeIcon class="size-3.5" />
            {{ t("tools.document.block.code") }}
          </DropdownMenuItem>
          <DropdownMenuItem
            class="gap-2 text-xs"
            @select="activeBlock && applyBlockChoice(activeBlock.id, 'divider', false)"
          >
            <MinusIcon class="size-3.5" />
            {{ t("tools.document.block.divider") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <!-- Export dropdown (.docx / .pdf) -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button
            variant="outline"
            size="xs"
            class="ms-auto h-7 gap-1 px-2 text-xs"
            :disabled="exporting"
          >
            <FileDownIcon class="size-3.5" />
            <span>{{ t("tools.document.export") }}</span>
            <ChevronDownIcon class="size-3 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-44">
          <DropdownMenuItem class="gap-2 text-xs" @select="handleExportDocx">
            <FileDownIcon class="size-3.5" />
            {{ t("tools.document.exportDocx") }}
          </DropdownMenuItem>
          <DropdownMenuItem class="gap-2 text-xs" @select="handleExportPdf">
            <PrinterIcon class="size-3.5" />
            {{ t("tools.document.exportPdf") }}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>

    <!-- Error / status bar -->
    <div
      v-if="error"
      class="shrink-0 border-b border-destructive/30 bg-destructive/10 px-3 py-1.5 text-[11px] text-destructive"
    >
      {{ error }}
    </div>
    <div
      v-else-if="externalUpdatePending"
      class="flex shrink-0 items-center justify-between gap-2 border-b border-warning/30 bg-warning/10 px-3 py-1 text-[11px] text-warning"
    >
      <span>{{ t("tools.document.externalUpdate") }}</span>
      <Button
        variant="outline"
        size="xs"
        class="h-5 px-2 text-[10px]"
        @click="loadDocument(currentPath)"
      >
        {{ t("tools.document.reload") }}
      </Button>
    </div>
    <div
      v-else-if="statusNotice"
      class="shrink-0 border-b border-primary/25 bg-primary/10 px-3 py-1 text-[11px] text-primary"
    >
      {{ statusNotice }}
    </div>

    <!-- Empty state -->
    <div
      v-if="!currentPath && !loading"
      class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center"
    >
      <div class="flex size-10 items-center justify-center rounded-xl border border-border bg-card text-primary">
        <FileTextIcon class="size-5" aria-hidden="true" />
      </div>
      <div class="max-w-xs space-y-1">
        <p class="text-xs font-semibold text-foreground">
          {{ t("tools.document.emptyTitle") }}
        </p>
        <p class="text-[11px] leading-relaxed text-muted-foreground">
          {{ t("tools.document.emptyDescription") }}
        </p>
      </div>
      <div class="flex items-center gap-2">
        <Button variant="default" size="sm" class="h-7 gap-1.5 text-xs" @click="handleOpen">
          <FolderOpenIcon class="size-3.5" />
          {{ t("tools.document.open") }}
        </Button>
        <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="handleNew">
          <FilePlus2Icon class="size-3.5" />
          {{ t("tools.document.new") }}
        </Button>
      </div>
    </div>

    <!-- Single-view Notion-style document canvas -->
    <div
      v-else
      class="min-h-0 flex-1 overflow-y-auto px-6 py-6 sm:px-10"
      :class="isMultiBlockDragging ? 'select-none' : ''"
      @click.self="clearBlockSelection(); blocks.length > 0 && focusBlock(blocks[blocks.length - 1].id, true)"
      @contextmenu="handleDocumentContextMenu"
    >
      <div class="mx-auto max-w-2xl space-y-1.5 pb-24">
        <div
          v-for="(block, idx) in blocks"
          :key="block.id"
          class="group relative rounded-md transition-colors"
          :class="
            selectedBlockIds.has(block.id)
              ? 'bg-primary/15 ring-1 ring-primary/35'
              : ''
          "
          @mousedown="handleBlockMouseDown(idx, $event)"
          @mouseenter="handleBlockMouseEnter(idx)"
        >
          <!-- Divider block -->
          <div
            v-if="block.kind === 'divider'"
            class="flex items-center gap-2 py-3"
          >
            <hr class="flex-1 border-border" />
            <Button
              variant="ghost"
              size="icon-xs"
              class="opacity-0 transition-opacity group-hover:opacity-100"
              @click="removeBlock(block.id)"
            >
              <Trash2Icon class="size-3 text-muted-foreground" />
            </Button>
          </div>

          <!-- Inline Image block -->
          <figure
            v-else-if="block.kind === 'image'"
            class="my-2 flex flex-col items-center gap-2 rounded-xl border border-border bg-card/60 p-3"
          >
            <div class="relative flex w-full justify-center">
              <img
                v-if="previewImageSrc(block.src)"
                :src="previewImageSrc(block.src)!"
                :alt="block.alt"
                class="max-h-80 max-w-full rounded-lg object-contain"
              />
              <span v-else class="py-4 font-mono text-xs text-muted-foreground">
                {{ block.src }}
              </span>
              <Button
                variant="ghost"
                size="icon-xs"
                class="absolute top-0 right-0 bg-background/80 opacity-0 transition-opacity group-hover:opacity-100"
                @click="removeBlock(block.id)"
              >
                <Trash2Icon class="size-3.5 text-muted-foreground" />
              </Button>
            </div>
            <input
              v-model="block.alt"
              type="text"
              :placeholder="t('tools.document.imageCaptionPlaceholder')"
              class="w-full bg-transparent text-center text-xs text-muted-foreground outline-hidden placeholder:text-muted-foreground/50"
            />
          </figure>

          <!-- Editable Table block -->
          <div
            v-else-if="block.kind === 'table'"
            class="my-2 overflow-x-auto rounded-lg border border-border bg-card/40"
          >
            <table class="w-full border-collapse text-xs">
              <thead>
                <tr class="border-b border-border bg-secondary/50">
                  <th
                    v-for="(_, hIdx) in block.headers"
                    :key="hIdx"
                    class="border-r border-border p-1.5 last:border-r-0"
                  >
                    <input
                      v-model="block.headers[hIdx]"
                      type="text"
                      class="w-full bg-transparent font-semibold text-foreground outline-hidden"
                    />
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="(row, rIdx) in block.rows"
                  :key="rIdx"
                  class="border-b border-border last:border-b-0"
                >
                  <td
                    v-for="(__, cIdx) in row"
                    :key="cIdx"
                    class="border-r border-border p-1.5 last:border-r-0"
                  >
                    <input
                      v-model="row[cIdx]"
                      type="text"
                      class="w-full bg-transparent text-foreground/90 outline-hidden"
                    />
                  </td>
                </tr>
              </tbody>
            </table>
            <div class="flex items-center gap-1 border-t border-border bg-secondary/30 px-2 py-1">
              <Button variant="ghost" size="xs" class="h-5 px-1.5 text-[10px]" @click="addTableRow(block)">
                + Row
              </Button>
              <Button variant="ghost" size="xs" class="h-5 px-1.5 text-[10px]" @click="addTableCol(block)">
                + Column
              </Button>
              <Button
                variant="ghost"
                size="xs"
                class="ms-auto h-5 px-1.5 text-[10px] text-muted-foreground hover:text-destructive"
                @click="removeBlock(block.id)"
              >
                <Trash2Icon class="size-3" />
              </Button>
            </div>
          </div>

          <!-- Text-based blocks (Paragraph, H1-H4, Bullet, Numbered, To-do, Quote, Code) -->
          <div
            v-else
            class="flex items-start gap-2 rounded-md px-1 py-0.5 transition-colors"
            :class="[
              block.kind === 'quote' ? 'border-l-2 border-primary/50 pl-3 my-1' : '',
              block.kind === 'code' ? 'rounded-lg border border-border bg-secondary/50 p-3 my-1.5' : '',
            ]"
          >
            <!-- List / To-do prefix markers -->
            <span
              v-if="block.kind === 'bullet'"
              class="mt-1.5 size-1.5 shrink-0 rounded-full bg-muted-foreground"
              aria-hidden="true"
            />
            <span
              v-else-if="block.kind === 'numbered'"
              class="mt-0.5 min-w-4 shrink-0 text-right font-mono text-xs text-muted-foreground tabular-nums"
            >
              {{ numberedIndexAt(idx) }}.
            </span>
            <input
              v-else-if="block.kind === 'todo'"
              v-model="block.checked"
              type="checkbox"
              class="mt-1 size-3.5 shrink-0 accent-primary"
            />

            <textarea
              :ref="(el) => setBlockInputRef(block.id, el)"
              :value="block.text"
              rows="1"
              :placeholder="blockPlaceholder(block.kind, idx)"
              class="h-auto min-h-[1lh] w-full min-w-0 flex-1 resize-none overflow-hidden border-0 bg-transparent p-0 [field-sizing:content] outline-hidden placeholder:text-muted-foreground/40"
              :class="[
                blockTextareaClass(block.kind),
                block.kind === 'todo' && block.checked ? 'line-through opacity-55' : '',
              ]"
              @focus="focusedBlockId = block.id"
              @input="handleBlockInput(block, $event)"
              @keydown="handleBlockKeydown(block, idx, $event)"
            />
          </div>

          <!-- Notion-style `/` Basic blocks slash menu -->
          <div
            v-if="slashMenuBlockId === block.id && filteredSlashItems.length > 0"
            class="absolute top-full left-0 z-30 mt-1 w-64 overflow-hidden rounded-xl border border-border bg-popover p-1 shadow-xl"
          >
            <p class="px-2.5 py-1 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
              {{ t("tools.document.basicBlocks") }}
            </p>
            <div class="max-h-64 overflow-y-auto">
              <button
                v-for="(item, itemIdx) in filteredSlashItems"
                :key="item.id"
                type="button"
                class="flex w-full items-center justify-between gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs transition-colors"
                :class="
                  itemIdx === slashActiveIndex
                    ? 'bg-accent text-accent-foreground'
                    : 'text-foreground hover:bg-accent/60'
                "
                @mousedown.prevent="applyBlockChoice(block.id, item.id, true)"
              >
                <span class="flex items-center gap-2.5">
                  <component :is="item.icon" class="size-3.5 shrink-0 text-muted-foreground" />
                  <span>{{ t(item.labelKey) }}</span>
                </span>
                <span v-if="item.shortcut" class="font-mono text-[10px] text-muted-foreground">
                  {{ item.shortcut }}
                </span>
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Selection status bar when multiple blocks are selected -->
    <div
      v-if="hasMultiBlockSelection"
      class="flex shrink-0 items-center justify-between gap-2 border-t border-border bg-card/80 px-3 py-1.5 text-[11px] text-muted-foreground"
    >
      <span class="font-medium text-foreground">
        {{ t("tools.document.selectionCount", { count: selectedBlockIds.size }) }}
      </span>
      <div class="flex items-center gap-1.5">
        <Button
          variant="default"
          size="xs"
          class="h-6 gap-1 px-2 text-[11px]"
          @click="
            ctxSelectedText = getSelectedBlocksMarkdown();
            runAgentAction('ask');
          "
        >
          <SparklesIcon class="size-3" />
          {{ t("tools.document.context.ask") }}
        </Button>
        <Button
          variant="outline"
          size="xs"
          class="h-6 gap-1 px-2 text-[11px]"
          @click="copySelectedDocumentText"
        >
          <CopyIcon class="size-3" />
          {{ t("context.copy") }}
        </Button>
      </div>
    </div>

    <!-- Right-click context menu on document selection -->
    <Teleport to="body">
      <div
        v-if="ctxMenuVisible"
        role="menu"
        class="fixed z-50 w-60 overflow-hidden rounded-xl border border-border bg-popover p-1 text-popover-foreground shadow-xl"
        :style="{ left: `${ctxMenuX}px`, top: `${ctxMenuY}px` }"
        @pointerdown.stop
        @contextmenu.prevent
      >
        <p class="px-2.5 py-1 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
          {{ t("tools.document.context.agentHeading") }}
        </p>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs font-medium text-primary hover:bg-accent hover:text-accent-foreground"
          @click="runAgentAction('ask')"
        >
          <MessageSquarePlusIcon class="size-3.5 shrink-0" />
          <span>{{ t("tools.document.context.ask") }}</span>
        </button>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="runAgentAction('explain')"
        >
          <BookOpenIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span>{{ t("tools.document.context.explain") }}</span>
        </button>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="runAgentAction('improve')"
        >
          <WandSparklesIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span>{{ t("tools.document.context.improve") }}</span>
        </button>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="runAgentAction('summarize')"
        >
          <SparklesIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span>{{ t("tools.document.context.summarize") }}</span>
        </button>

        <div class="my-1 h-px bg-border" role="separator" />

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="copySelectedDocumentText"
        >
          <CopyIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span>{{ t("context.copy") }}</span>
        </button>

        <button
          type="button"
          role="menuitem"
          class="flex w-full items-center gap-2 rounded-lg px-2.5 py-1.5 text-left text-xs hover:bg-accent hover:text-accent-foreground"
          @click="closeContextMenu(); selectAllBlocks()"
        >
          <TextSelectIcon class="size-3.5 shrink-0 text-muted-foreground" />
          <span>{{ t("tools.document.context.selectDocument") }}</span>
        </button>
      </div>
    </Teleport>
  </div>
</template>
