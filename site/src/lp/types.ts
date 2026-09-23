// The landing page is one layout fed by one object of words per language.
// Nothing in the components says anything itself: every string comes from here,
// so the English page is the same components with copy.en.ts.

export type Link = { label: string; href: string; external?: boolean };

export type Scene = {
  id: string;
  tab: string;
  title: string;
  body: string;
  points: string[];
  image: { src: string; alt: string; width: number; height: number };
};

export type Feature = {
  id: string;
  tone: "purple" | "green" | "yellow" | "blue";
  eyebrow: string;
  title: string;
  body: string;
  points: string[];
  image: { src: string; alt: string };
};

export type LpCopy = {
  lang: "ja" | "en";
  meta: { title: string; description: string; ogImage: string };
  nav: { links: Link[]; cta: string; menu: string };
  hero: {
    badge: string;
    title: string[];
    lead: string;
    install: { label: string; command: string; copy: string; copied: string };
    store: string;
    fineprint: string;
    zip: Link;
    github: Link;
    works: { label: string; names: string[] };
    image: { src: string; alt: string };
  };
  scenes: { eyebrow: string; title: string; items: Scene[] };
  problem: {
    eyebrow: string;
    title: string;
    body: string[];
    image: { src: string; alt: string };
  };
  solution: {
    eyebrow: string;
    title: string;
    body: string[];
    states: { working: string; done: string; waiting: string };
    image: { src: string; alt: string };
  };
  features: { eyebrow: string; title: string; items: Feature[] };
  phone: {
    eyebrow: string;
    title: string;
    body: string[];
    image: { src: string; alt: string };
    shot: { src: string; alt: string };
    link: Link;
  };
  trust: {
    eyebrow: string;
    title: string;
    items: { title: string; body: string; link?: Link }[];
    stars: { label: string; ask: string; button: string; why: string };
  };
  steps: {
    eyebrow: string;
    title: string;
    items: { title: string; body: string }[];
    image: { src: string; alt: string };
  };
  faq: { eyebrow: string; title: string; items: { q: string; a: string }[] };
  closing: { title: string; store: string; fineprint: string; image: { src: string; alt: string } };
  footer: { columns: { title: string; links: Link[] }[]; note: string };
};
