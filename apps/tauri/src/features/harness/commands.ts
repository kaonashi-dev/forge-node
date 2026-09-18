import { sendRuntimeCommand } from "../../runtime/host";

/** Read output written before a job viewer was opened. */
export async function readJobLog(jobId: string): Promise<void> {
  await sendRuntimeCommand({ type: "read_job_log", job_id: jobId });
}
