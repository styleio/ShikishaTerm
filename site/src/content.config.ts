import { defineCollection } from "astro:content";
import { docsLoader, i18nLoader } from "@astrojs/starlight/loaders";
import { docsSchema, i18nSchema } from "@astrojs/starlight/schema";

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
  // Starlight looks this collection up on every build for overrides of its own
  // UI strings, and warns when it is missing or empty. `src/content/i18n/en.json`
  // is an empty object for that reason alone: the site keeps Starlight's words
  i18n: defineCollection({ loader: i18nLoader(), schema: i18nSchema() }),
};
