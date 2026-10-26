import { imageDataUrl, pasteClipboardImage, saveImage } from "./api";

/** How a draft refers to a pasted image until submitting uploads it. */
export const IMAGE_SCHEME = "wispy-image:";

/** The images on the clipboard of a paste, if any. */
export function pastedImages(e: React.ClipboardEvent) {
  return [...e.clipboardData.items].filter((item) => item.kind === "file" && item.type.startsWith("image/")).flatMap((item) => item.getAsFile() ?? []);
}

/**
 * Pastes clipboard images into a text box as Markdown (stored locally, uploaded on submit);
 * text pastes as usual. `onError` gets refusals ("" once a paste worked).
 */
export function pasteImages(e: React.ClipboardEvent<HTMLTextAreaElement>, onError: (message: string) => void) {
  const files = pastedImages(e);
  const el = e.currentTarget;
  // WebKitGTK leaves images out of the paste event; the app reads them from the clipboard then.
  if (files.length) e.preventDefault();
  void (async () => {
    try {
      const markdown = files.length ? await Promise.all(files.map(async (file) => saveImage(file.type, await base64(file)))) : [await pasteClipboardImage()];
      const text = markdown.filter(Boolean).join("\n");
      if (!text) return;
      el.focus();
      // Typed in, so ⌘Z undoes it and React's onChange sees it.
      if (!document.execCommand("insertText", false, text)) el.setRangeText(text, el.selectionStart, el.selectionEnd, "end");
      onError("");
    } catch (err) {
      onError(String(err));
    }
  })();
}

async function base64(file: Blob) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

const loading = new Map<string, Promise<void>>();
/** Loaded images' `data:` URLs (null: no such image). */
export const imageUrls = new Map<string, string | null>();

/** Loads pasted images into `imageUrls` (each once). */
export function loadImages(ids: string[]) {
  return Promise.all(
    ids.map((id) => {
      let done = loading.get(id);
      if (!done) {
        done = imageDataUrl(id)
          .catch(() => null)
          .then((url) => void imageUrls.set(id, url));
        loading.set(id, done);
      }
      return done;
    }),
  );
}
