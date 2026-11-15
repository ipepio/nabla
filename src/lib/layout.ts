/**
 * Layout geometry shared by the composer and the resizable right panel.
 *
 * The prompt box has a normal width of `COMPOSER_MAX_WIDTH` and may shrink to
 * half of it (`COMPOSER_MIN_WIDTH`). That minimum, plus the wrapper padding of
 * the chat column, is the floor the chat section may never cross — which is also
 * the ceiling for the right panel (spec: the panel resizes freely until the chat
 * reaches its own minimum).
 *
 * One module so the three numbers cannot drift apart again: the composer states
 * its bounds from here, and `ResizableAside` derives its ceiling from here.
 */

/** Normal (and maximum) width of the prompt box. */
export const COMPOSER_MAX_WIDTH = 640;

/** Half of the normal width: the narrowest the prompt box may get. */
export const COMPOSER_MIN_WIDTH = COMPOSER_MAX_WIDTH / 2;

/** Horizontal padding of the chat column that hosts the composer (`px-4`). */
const CHAT_COLUMN_PADDING = 32;

/** Floor for the whole chat section: composer minimum plus its padding. */
export const CHAT_MIN_WIDTH = COMPOSER_MIN_WIDTH + CHAT_COLUMN_PADDING;
