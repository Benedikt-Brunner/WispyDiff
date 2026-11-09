/**
 * Back / forward through the places a jump left, like a browser's history: a jump records where the
 * reader was; going back records where they are for going forward again. Places equal to where the
 * reader is (e.g. a read-only file view closed with Esc), and ones that can't be shown any more, are
 * skipped.
 */
export class NavHistory<T> {
  private back: T[] = [];
  private forward: T[] = [];

  constructor(
    private readonly same: (a: T, b: T) => boolean,
    private readonly limit = 100,
  ) {}

  /** Before a jump: remember `here`; a new jump drops the places gone back from. */
  push(here: T) {
    const last = this.back[this.back.length - 1];
    // Close to the last place: that one moves here.
    if (last !== undefined && this.same(last, here)) this.back.pop();
    this.back.push(here);
    if (this.back.length > this.limit) this.back.shift();
    this.forward = [];
  }

  /** The place to go back to from `here` (which forward then returns to), if any. */
  goBack(here: T, usable: (place: T) => boolean = () => true): T | undefined {
    return this.step(this.back, this.forward, here, usable);
  }

  goForward(here: T, usable: (place: T) => boolean = () => true): T | undefined {
    return this.step(this.forward, this.back, here, usable);
  }

  private step(from: T[], to: T[], here: T, usable: (place: T) => boolean): T | undefined {
    let place = from.pop();
    while (place !== undefined && (this.same(place, here) || !usable(place))) place = from.pop();
    if (place !== undefined) to.push(here);
    return place;
  }
}
