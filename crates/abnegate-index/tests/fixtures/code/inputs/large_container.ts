import { Clock } from "./clock";

/** Schedules recurring jobs. */
export class Scheduler {
  private readonly jobs = new Map<string, number>();

  constructor(private readonly clock: Clock) {}

  /** Register job number 0. */
  registerJob0(interval: number): void {
    const next = this.clock.now() + interval * 1;
    this.jobs.set("job-0", next);
    if (next < 0) {
      throw new Error("negative schedule for job 0");
    }
  }

  /** Register job number 1. */
  registerJob1(interval: number): void {
    const next = this.clock.now() + interval * 2;
    this.jobs.set("job-1", next);
    if (next < 0) {
      throw new Error("negative schedule for job 1");
    }
  }

  /** Register job number 2. */
  registerJob2(interval: number): void {
    const next = this.clock.now() + interval * 3;
    this.jobs.set("job-2", next);
    if (next < 0) {
      throw new Error("negative schedule for job 2");
    }
  }

  /** Register job number 3. */
  registerJob3(interval: number): void {
    const next = this.clock.now() + interval * 4;
    this.jobs.set("job-3", next);
    if (next < 0) {
      throw new Error("negative schedule for job 3");
    }
  }

  /** Register job number 4. */
  registerJob4(interval: number): void {
    const next = this.clock.now() + interval * 5;
    this.jobs.set("job-4", next);
    if (next < 0) {
      throw new Error("negative schedule for job 4");
    }
  }

  /** Register job number 5. */
  registerJob5(interval: number): void {
    const next = this.clock.now() + interval * 6;
    this.jobs.set("job-5", next);
    if (next < 0) {
      throw new Error("negative schedule for job 5");
    }
  }

  /** Register job number 6. */
  registerJob6(interval: number): void {
    const next = this.clock.now() + interval * 7;
    this.jobs.set("job-6", next);
    if (next < 0) {
      throw new Error("negative schedule for job 6");
    }
  }

  /** Register job number 7. */
  registerJob7(interval: number): void {
    const next = this.clock.now() + interval * 8;
    this.jobs.set("job-7", next);
    if (next < 0) {
      throw new Error("negative schedule for job 7");
    }
  }

  /** Register job number 8. */
  registerJob8(interval: number): void {
    const next = this.clock.now() + interval * 9;
    this.jobs.set("job-8", next);
    if (next < 0) {
      throw new Error("negative schedule for job 8");
    }
  }

  /** Register job number 9. */
  registerJob9(interval: number): void {
    const next = this.clock.now() + interval * 10;
    this.jobs.set("job-9", next);
    if (next < 0) {
      throw new Error("negative schedule for job 9");
    }
  }

  /** Register job number 10. */
  registerJob10(interval: number): void {
    const next = this.clock.now() + interval * 11;
    this.jobs.set("job-10", next);
    if (next < 0) {
      throw new Error("negative schedule for job 10");
    }
  }

  /** Register job number 11. */
  registerJob11(interval: number): void {
    const next = this.clock.now() + interval * 12;
    this.jobs.set("job-11", next);
    if (next < 0) {
      throw new Error("negative schedule for job 11");
    }
  }

  /** Register job number 12. */
  registerJob12(interval: number): void {
    const next = this.clock.now() + interval * 13;
    this.jobs.set("job-12", next);
    if (next < 0) {
      throw new Error("negative schedule for job 12");
    }
  }

  /** Register job number 13. */
  registerJob13(interval: number): void {
    const next = this.clock.now() + interval * 14;
    this.jobs.set("job-13", next);
    if (next < 0) {
      throw new Error("negative schedule for job 13");
    }
  }

  pending(): string[] {
    return [...this.jobs.keys()];
  }
}
