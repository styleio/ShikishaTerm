// The Markdown kit the board carries: one file, built by tools/mdkit/build.mjs
// into vendor/mdkit/mdkit.js and handed to the page as `window.MdKit`.
//
// Three things, and nothing about the screen around them:
//   render()   Markdown to safe HTML, with the front matter apart, the headings
//              listed, and every block told which lines of the file it came from
//   rich()     a visual editor over a Markdown text, and the text back out
//   keep()     the text written back so that what was not edited keeps the
//              spelling it had (list marks, blank lines, line ends)
// The page decides where any of it goes.

import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkGfm from 'remark-gfm';
import remarkCjkFriendly from 'remark-cjk-friendly';
import remarkBreaks from 'remark-breaks';
import remarkMath from 'remark-math';
import remarkRehype from 'remark-rehype';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import rehypeSlug from 'rehype-slug';
import rehypeHighlight from 'rehype-highlight';
import rehypeKatex from 'rehype-katex';
import rehypeStringify from 'rehype-stringify';
import { visit } from 'unist-util-visit';
import { toString as mdText } from 'mdast-util-to-string';
import GithubSlugger from 'github-slugger';
import DOMPurify from 'dompurify';
import DiffMatchPatch from 'diff-match-patch';

import { Editor } from '@tiptap/core';
import StarterKit from '@tiptap/starter-kit';
import { Markdown } from '@tiptap/markdown';
import { TableKit } from '@tiptap/extension-table';
import { TaskList, TaskItem } from '@tiptap/extension-list';
import Image from '@tiptap/extension-image';
import CodeBlockLowlight from '@tiptap/extension-code-block-lowlight';
import { Mathematics } from '@tiptap/extension-mathematics';
import { Placeholder } from '@tiptap/extensions';
import { createLowlight, common } from 'lowlight';

// ── Front matter ────────────────────────────────────────────────────────────

// The block at the very top between two `---` lines (YAML) or two `+++`
// lines (TOML), apart from the body. `lines` is how many lines it takes, with
// its fences, so a block's line in the body can be said as a line of the file
export function splitFront(text) {
  const src = String(text || '');
  const m = /^(---|\+\+\+)[ \t]*\r?\n([\s\S]*?)\r?\n\1[ \t]*(?:\r?\n|$)/.exec(src);
  if (!m) return { front: null, body: src, lines: 0 };
  const lines = m[0].split(/\r?\n/).length - (m[0].endsWith('\n') ? 1 : 0);
  return { front: m[2], fence: m[1], raw: m[0], body: src.slice(m[0].length), lines };
}

// ── Reading ─────────────────────────────────────────────────────────────────

// What a document may hold once read. GitHub's own list, and on top of it the
// classes the maths and the code colouring are carried in -- they are added
// before the cleaning, so the cleaning has to let them through
const schema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    code: [...(defaultSchema.attributes.code || []), ['className', /^language-./, 'math-inline', 'math-display']],
    span: [...(defaultSchema.attributes.span || []), ['className', 'math-inline', 'math-display']],
    div: [...(defaultSchema.attributes.div || []), ['className', 'math-display']],
    th: [...(defaultSchema.attributes.th || []), 'align'],
    td: [...(defaultSchema.attributes.td || []), 'align'],
  },
  // A link to a file beside the document is a link the page opens itself
  protocols: { ...defaultSchema.protocols, href: [...(defaultSchema.protocols.href || []), 'file'] },
};

// Every block remembers the lines of the file it was written on, so a note
// written beside it can say where it is. Counted from the top of the file,
// front matter included
const BLOCKS = new Set(['p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'li', 'blockquote', 'pre', 'table', 'tr', 'hr', 'details', 'div']);
function rehypeLines(offset) {
  return tree => {
    visit(tree, 'element', node => {
      if (!BLOCKS.has(node.tagName) || !node.position) return;
      node.properties = node.properties || {};
      node.properties.dataFrom = node.position.start.line + offset;
      node.properties.dataTo = node.position.end.line + offset;
    });
  };
}

function reader(offset, words = {}) {
  return unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(remarkCjkFriendly)
    // A line break in the file is a line break on the screen. Notes, chat and
    // AI answers are written that way, and reading them joined is reading
    // something the writer did not write
    .use(remarkBreaks)
    .use(remarkMath)
    .use(remarkRehype, {
      allowDangerousHtml: true,
      // The words the footnotes are headed and linked back with, in the
      // reader's language
      ...(words.footnotes ? { footnoteLabel: words.footnotes } : {}),
      ...(words.back ? { footnoteBackLabel: words.back } : {}),
    })
    .use(rehypeRaw)
    .use(rehypeSanitize, schema)
    .use(rehypeLines, offset)
    .use(rehypeSlug)
    .use(rehypeHighlight, { detect: false, plainText: ['mermaid', 'text', 'txt', 'plain'] })
    .use(rehypeKatex, { throwOnError: false, strict: 'ignore' })
    .use(rehypeStringify);
}

// The headings, for the contents: their level, words, the id the page gives
// them (the same one rehype-slug gives, so a press finds it) and their line
export function headings(body, offset = 0) {
  const tree = unified().use(remarkParse).use(remarkGfm).use(remarkMath).parse(body);
  const slugger = new GithubSlugger();
  const out = [];
  visit(tree, 'heading', node => {
    const text = mdText(node);
    out.push({ depth: node.depth, text, id: slugger.slug(text), line: (node.position ? node.position.start.line : 0) + offset });
  });
  return out;
}

// A document, read: its HTML (cleaned), its front matter apart, and how many
// lines that took. Throws only if the libraries do; the page shows the text
export function render(text, words) {
  const { front, body, lines } = splitFront(text);
  const html = String(reader(lines, words).processSync(body));
  return { html, front, frontLines: lines };
}

// What a diagram's picture may hold: SVG and nothing that runs
export function cleanSvg(svg) {
  return DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } });
}

// ── Writing ─────────────────────────────────────────────────────────────────

const lowlight = createLowlight(common);

// What the page lends the kit: how a diagram is drawn (the page loads the
// diagram library when one is first wanted) and the words on the controls
export const hooks = { diagram: null, words: { copy: 'Copy', copied: 'Copied', language: 'Language', plain: 'Plain text' } };

// The languages a code block offers, by the name Markdown writes after ```
export const LANGUAGES = ['mermaid', ...lowlight.listLanguages().sort()];

// A code block in the visual editor: the code, a choice of its language and a
// copy button above it, and for a diagram, the picture under the code as it
// is typed
const CodeBlock = CodeBlockLowlight.extend({
  addNodeView() {
    return ({ node, editor, getPos }) => {
      const dom = document.createElement('div');
      dom.className = 'mdcode';
      const bar = document.createElement('div');
      bar.className = 'mdcodebar';
      bar.contentEditable = 'false';
      const pick = document.createElement('select');
      pick.title = hooks.words.language;
      for (const name of ['', ...LANGUAGES]) {
        const o = document.createElement('option');
        o.value = name;
        o.textContent = name || hooks.words.plain;
        pick.append(o);
      }
      pick.onchange = () => {
        const at = getPos();
        if (typeof at !== 'number') return;
        editor.view.dispatch(editor.state.tr.setNodeMarkup(at, undefined, { ...current.attrs, language: pick.value || null }));
      };
      const copy = document.createElement('button');
      copy.type = 'button';
      copy.textContent = hooks.words.copy;
      copy.onclick = () => {
        navigator.clipboard && navigator.clipboard.writeText(current.textContent);
        copy.textContent = hooks.words.copied;
        setTimeout(() => { copy.textContent = hooks.words.copy; }, 1200);
      };
      bar.append(pick, copy);
      const pre = document.createElement('pre');
      const code = document.createElement('code');
      pre.append(code);
      const picture = document.createElement('div');
      picture.className = 'mddiagram';
      picture.contentEditable = 'false';
      dom.append(bar, pre, picture);
      let current = node;
      let drawn = null;
      const show = n => {
        current = n;
        const lang = n.attrs.language || '';
        pick.value = LANGUAGES.includes(lang) ? lang : '';
        code.className = lang ? 'language-' + lang : '';
        const diagram = lang === 'mermaid';
        picture.hidden = !diagram;
        if (diagram && hooks.diagram && drawn !== n.textContent) {
          drawn = n.textContent;
          hooks.diagram(picture, n.textContent);
        }
      };
      show(node);
      return {
        dom,
        contentDOM: code,
        update: n => { if (n.type !== node.type) return false; show(n); return true; },
        // The controls are the page's, not the document's
        stopEvent: e => bar.contains(e.target),
        ignoreMutation: m => !code.contains(m.target),
      };
    };
  },
});

// A visual editor over `markdown` in `el`. `onChange` hears that something
// was typed; the text is asked for with `markdown()` when it is wanted, so a
// long document is not written out on every key
export function rich(el, { markdown = '', placeholder = '', onChange = null, onSelect = null, editable = true } = {}) {
  const editor = new Editor({
    element: el,
    editable,
    extensions: [
      StarterKit.configure({ codeBlock: false, link: { openOnClick: false, autolink: true, linkOnPaste: true } }),
      CodeBlock.configure({ lowlight, defaultLanguage: null }),
      Markdown.configure({ markedOptions: { gfm: true } }),
      TableKit.configure({ table: { resizable: false } }),
      TaskList,
      TaskItem.configure({ nested: true }),
      Image.configure({ inline: true }),
      Mathematics.configure({ katexOptions: { throwOnError: false } }),
      Placeholder.configure({ placeholder }),
    ],
    content: markdown,
    contentType: 'markdown',
    onUpdate: () => onChange && onChange(),
    onSelectionUpdate: () => onSelect && onSelect(),
  });
  return {
    editor,
    markdown: () => editor.getMarkdown(),
    set: md => editor.commands.setContent(md, { contentType: 'markdown', emitUpdate: false }),
    destroy: () => editor.destroy(),
  };
}

// Text as the visual editor would write it back, without an editor on screen:
// what "nothing was changed" looks like for this document
export function normalise(markdown) {
  const host = document.createElement('div');
  const r = rich(host, { markdown, editable: false });
  const out = r.markdown();
  r.destroy();
  return out;
}

// Whether two texts read the same: the same HTML once read. What the visual
// editor writes may spell a list or an emphasis differently and still be the
// same document; a link or a table lost is not the same document
export function sameReading(a, b) {
  const norm = h => h.replace(/\sdata-(from|to)="\d+"/g, '').replace(/\s+/g, ' ').trim();
  try { return norm(render(a).html) === norm(render(b).html); } catch { return false; }
}

// Whether a document can be opened in the visual editor without the editor
// changing what it says: read in, written out, read again, the same. `null`
// when it can; the reason when it cannot
export function richCheck(text) {
  const { body } = splitFront(text);
  let back;
  try { back = normalise(body); } catch (e) { return 'unreadable'; }
  return sameReading(body, back) ? null : 'changes';
}

// The visual editor's text written back over what was on the disk, keeping
// the spelling of everything that was not edited. Three texts: the file as
// read (`base`), the editor's reading of it (`was`), and the editor's text now
// (`now`). What changed between `was` and `now` is put onto `base`; if the
// result does not read the same as `now`, the editor's own text is used --
// keeping a spelling is never worth changing what the document says
const dmp = new DiffMatchPatch();
dmp.Diff_Timeout = 0.05;
export function keep(base, was, now) {
  if (now === was) return base;
  const patches = dmp.patch_make(was, now);
  const [merged, applied] = dmp.patch_apply(patches, base);
  const out = applied.every(Boolean) && sameReading(merged, now) ? merged : now;
  return shapedLike(base, out);
}

// The file's own line ends, and as many of them at its end as it had: the
// editor writes LF and its own number of blank lines after a table, and
// neither is something the person changed
function shapedLike(base, text) {
  const crlf = /\r\n/.test(base);
  let out = crlf ? text.replace(/\r?\n/g, '\r\n') : text.replace(/\r\n/g, '\n');
  const tail = /(?:\r?\n[ \t]*)*$/.exec(base)[0].replace(/[ \t]/g, '');
  return out.replace(/(?:\r?\n[ \t]*)*$/, '') + tail;
}

export const version = 1;
