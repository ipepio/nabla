/**
 * Layout geometry shared by the composer, the resizable right panel and the
 * window itself.
 *
 * The prompt box is the anchor: everything else is expressed from it.
 *
 * 1. The box is `COMPOSER_MAX_WIDTH` normally and may shrink to half of it
 *    (`COMPOSER_MIN_WIDTH`).
 * 2. That minimum plus the chat column padding is the floor the chat section may
 *    never cross (`CHAT_MIN_WIDTH`); it is also the ceiling for the right panel.
 * 3. The application window cannot get narrower than the sum of everything that
 *    cannot shrink around the chat: the left drawer, the chat floor and the
 *    right panel at its minimum (`APP_MIN_WIDTH`). Below that point the prompt
 *    box would have to go under its own minimum, so the window stops first.
 *
 * One module for all of it, because these numbers drifted three times when they
 * were literals in component styles.
 *
 * `src-tauri/tauri.conf.json` mirrors `APP_MIN_WIDTH`/`APP_MIN_HEIGHT` in its
 * window `minWidth`/`minHeight` (the config is static JSON and cannot import
 * this file); keep the two in sync when changing the values here.
 */

/** Normal (and maximum) width of the prompt box. */
export const COMPOSER_MAX_WIDTH = 640;

/** Half of the normal width: the narrowest the prompt box may get. */
export const COMPOSER_MIN_WIDTH = COMPOSER_MAX_WIDTH / 2;

/** Horizontal padding of the chat column that hosts the composer (`px-4`). */
const CHAT_COLUMN_PADDING = 32;

/** Floor for the whole chat section: composer minimum plus its padding. */
export const CHAT_MIN_WIDTH = COMPOSER_MIN_WIDTH + CHAT_COLUMN_PADDING;

/** Left drawer at its default (expanded) width — shadcn `--sidebar-width`. */
const SIDEBAR_WIDTH = 256;

/** Narrowest the right tools panel may get (`ResizableAside` MIN_WIDTH). */
const PANEL_MIN_WIDTH = 240;

/** Minimum window width: the prompt box at half width, plus its neighbours. */
export const APP_MIN_WIDTH = SIDEBAR_WIDTH + CHAT_MIN_WIDTH + PANEL_MIN_WIDTH;

/** Minimum window height, in logical pixels. */
export const APP_MIN_HEIGHT = 640;
