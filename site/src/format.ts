// Text the page builds from values: a message's {names} filled in, and a size. Kept apart from i18n/index.ts, which reads the browser's languages, so
// it runs anywhere (test/format.test.ts).

/** A message with its {names} filled in with text. A name the message has and
 *  `values` does not is an error, so no "{size}" reaches a visitor. */
export function fill(message: string, values: Record<string, string>): string {
  return message.replace(/\{(\w+)\}/g, (_, name: string) => {
    const value = values[name];
    if (value === undefined) throw new Error(`a message names {${name}}, which is not given: ${message}`);
    return value;
  });
}

/** Bytes as Finder counts them: 7438342 is "7.4 MB". */
export function megabytes(size: number, locale: string): string {
  return `${new Intl.NumberFormat(locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(size / 1e6)} MB`;
}
