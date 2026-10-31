/**
 * Start cancelling a browser download before resolving its filename pipeline.
 *
 * Chrome's filename pipeline waits for suggest. Request cancellation before
 * releasing that pipeline so Save As can never be scheduled for a download that
 * FluxDown takes over.
 *
 * `downloads.cancel` takes effect synchronously in the browser, and Chrome
 * rejects `suggest` for a download that is no longer in progress with
 * "Download must be in progress" — reported as an unchecked `runtime.lastError`
 * because the binding gives us no callback. So suggest is only released while
 * the download is still in progress. The state query is issued right after the
 * cancel request; the browser handles extension calls in order, so it observes
 * the post-cancel state without waiting for the cancel reply. An interrupted
 * download's pending filename decision is dropped by Chrome (erase removes the
 * item; otherwise its determiner timeout ends it).
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
  stateOf: (downloadId: number) => Promise<string | undefined>,
): Promise<boolean> {
  let cancellation: Promise<void>;
  try {
    cancellation = cancel(downloadId);
  } catch {
    resolveFilename();
    return false;
  }

  stateOf(downloadId).then(
    (state) => {
      if (state === "in_progress") resolveFilename();
    },
    // Unknown state: release the pipeline rather than risk holding it.
    () => resolveFilename(),
  );

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
