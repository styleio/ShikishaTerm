// Substitute only the original placeholders; a value is always literal text.
function fill(text, args) {
  return String(text ?? "").replace(/\{([^{}]+)\}/g,
    (whole, key) => Object.prototype.hasOwnProperty.call(args, key) ? String(args[key]) : whole);
}
