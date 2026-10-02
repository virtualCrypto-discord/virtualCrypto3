import ja from "./locales/ja.json" with { type: "json" };

export type MessageKey = keyof typeof ja;
export type Catalog = Partial<Record<MessageKey, string>>;

/** Japanese remains the fallback while additional catalogs are being translated. */
export function createTranslator(catalog: Catalog = {}) {
  return (key: MessageKey, values: Record<string, string | number> = {}): string => {
    const template = catalog[key] ?? ja[key];
    return template.replace(/\{(\w+)\}/g, (placeholder, name: string) =>
      Object.hasOwn(values, name) ? String(values[name]) : placeholder,
    );
  };
}

export const t = createTranslator();
