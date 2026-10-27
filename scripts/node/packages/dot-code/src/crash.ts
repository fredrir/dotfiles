export class CrashTracker {
  private readonly maxCrashes = 5;
  private readonly windowMs = 3 * 60 * 1000; // 3 minutes
  private crashTimestamps: number[] = [];

  public recordCrash(): { isCrashLoop: boolean; crashCount: number } {
    const now = Date.now();
    this.crashTimestamps = this.crashTimestamps.filter(
      (ts) => now - ts < this.windowMs,
    );
    this.crashTimestamps.push(now);
    return {
      isCrashLoop: this.crashTimestamps.length >= this.maxCrashes,
      crashCount: this.crashTimestamps.length,
    };
  }

  public reset(): void {
    this.crashTimestamps = [];
  }
}
