// The page's languages. A visitor gets their own earlier choice, else the
// first of their browser's languages the page speaks, else English: the applet
// store's website picks the same way (pocket-nexus/applets,
// store/site/src/i18n/index.ts), so a visitor it sends here reads on in the
// language they were reading.
//
// The words are plain objects, one file per language, held to en.ts's shape by
// the Wording type: one page with no plurals has no use for a message library.
//
// To add a language: a file here shaped like en.ts, and a line in LOCALES and
// in WORDING.
import { computed, ref, watch } from "vue";
import en from "./en.ts";
import zhCN from "./zh-CN.ts";

/** What every language's file must hold: en.ts's keys, each with a message. */
export type Wording = Shape<typeof en>;
type Shape<T> = { [K in keyof T]: T[K] extends string ? string : Shape<T[K]> };

/** The languages, each by its own name for itself. */
export const LOCALES = [
  { id: "en", name: "English", short: "EN" },
  { id: "zh-CN", name: "简体中文", short: "中文" },
] as const;

export type Locale = (typeof LOCALES)[number]["id"];

const WORDING: Record<Locale, Wording> = { en, "zh-CN": zhCN };
const FALLBACK: Locale = "en";
/** Where a visitor's own choice is kept. */
const CHOICE = "locale";

const speaks = (id: unknown): id is Locale => LOCALES.some((locale) => locale.id === id);

function chosen(): Locale | undefined {
  try {
    const kept = localStorage.getItem(CHOICE);
    return speaks(kept) ? kept : undefined;
  } catch {
    return undefined;
  }
}

/** The first of the browser's languages the page speaks: its exact name, or
 *  the one language it shares (zh-TW reads zh-CN sooner than English). */
function preferred(): Locale {
  for (const wanted of navigator.languages ?? [navigator.language]) {
    if (speaks(wanted)) return wanted;
    const language = wanted.split("-")[0];
    const near = LOCALES.find((locale) => locale.id.split("-")[0] === language);
    if (near) return near.id;
  }
  return FALLBACK;
}

const locale = ref<Locale>(chosen() ?? preferred());
const wording = computed(() => WORDING[locale.value]);

// the document says which language it is in, to a screen reader and to the
// browser, and is named in it
watch(locale, (now) => {
  document.documentElement.lang = now;
  document.title = WORDING[now].title;
}, { immediate: true });

/** The visitor picks a language: the page changes now, and stays that way. */
export function chooseLocale(chosen: Locale) {
  locale.value = chosen;
  try {
    localStorage.setItem(CHOICE, chosen);
  } catch {
    // a browser that keeps nothing: the choice lasts the visit
  }
}

/** The words of the visitor's language, and the language itself. */
export function useLanguage() {
  return { t: wording, locale };
}
