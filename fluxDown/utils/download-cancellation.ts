/**
 * Start cancelling a browser download before resolving its filename pipeline.
 *
 * Chrome's filename pipeline waits for suggest. Request cancellation before
 * releasing that pipeline, and avoid holding suggest until cancellation settles:
 * a pending filename decision could otherwise block cancellation progress.
 *
 * Resolves as soon as the cancellation settles. Erasing only removes the
 * cancelled entry from the browser's download list, so it runs in the
 * background instead of delaying the hand-off to FluxDown.
 */
export async function cancelBeforeFilenameResolution(
  downloadId: number,
  cancel: (downloadId: number) => Promise<void>,
  erase: (query: { id: number }) => Promise<unknown>,
  resolveFilename: () => void,
): Promise<boolean> {
  let cancellation: Promise<void>;
  try {
    cancellation = cancel(downloadId);
  } catch {
    resolveFilename();
    return false;
  }

  resolveFilename();

  try {
    await cancellation;
  } catch {
    return false;
  }

  Promise.resolve()
    .then(() => erase({ id: downloadId }))
    .catch((error: unknown) => {
      // The cancellation succeeded; a leftover history entry is cosmetic.
      console.debug("[FluxDown] failed to erase cancelled download:", downloadId, error);
    });
  return true;
}
