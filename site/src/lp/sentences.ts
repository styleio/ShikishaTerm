// A title of two sentences breaks between them first: each sentence is kept
// together when it fits (see .sentence in lp.css), so a line never ends in the
// middle of the first one. Only the Japanese full stop splits; English titles
// come back whole and wrap as they always did.
export const sentences = (title: string) => title.split(/(?<=。)/).filter(Boolean);
