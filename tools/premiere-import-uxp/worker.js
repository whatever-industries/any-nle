const { ppro, fs, guid, importOriginals } = require("./capture.js");
const schema = "anynle.premiere-worker/2";
const interval = 1000;
const prefix = "job-";
const maxRequest = 65536;

async function record(folder, name, value, overwrite = false) {
  const file = await folder.createFile(name + ".tmp", { overwrite: true });
  await file.write(JSON.stringify({ schema, ...value }));
  await file.moveTo(folder, { newName: name, overwrite });
}

async function entries(folder) {
  return new Map((await folder.getEntries()).map((entry) => [entry.name, entry]));
}

async function readJob(folder) {
  const files = await entries(folder);
  if (files.has("claimed.json") || files.has("result.json")) return null;
  const entry = files.get("request.json");
  if (!entry?.isFile) return null;
  const { size } = await entry.getMetadata();
  if (size > maxRequest) return null;
  const request = JSON.parse(await entry.read());
  if (request.schema !== schema || !guid.test(request.id)
      || folder.name !== prefix + request.id || !Number.isSafeInteger(request.expires)
      || request.expires <= Date.now() || request.expires > Date.now() + 45_000
      || !Array.isArray(request.originals) || !request.originals.length
      || request.originals.length > 64
      || !request.originals.every((path) => typeof path === "string"
          && path.startsWith("/") && !/[\0\r\n]/.test(path))
      || new Set(request.originals).size !== request.originals.length) return null;
  return request;
}

async function hasOpenProject() {
  return Boolean(await ppro.Project.getActiveProject())
    || (await ppro.ProjectUtils.getProjectViewIds()).length > 0;
}

async function execute(folder, request) {
  await record(folder, "claimed.json", { id: request.id });
  const scratch = folder.nativePath + "/Any NLE Import Worker " + request.id + ".prproj";
  const donor = folder.nativePath + "/Any NLE Import " + request.id + "/Donor.prproj";
  let project;
  let imported;
  let error;
  try {
    if (await hasOpenProject()) {
      throw new Error("Close the open Premiere project before using assisted linking.");
    }
    if ((await entries(folder)).has(scratch.split("/").pop())) {
      throw new Error("Scratch path is already occupied.");
    }
    if (Date.now() >= request.expires) throw new Error("The request expired.");
    if (await hasOpenProject()) throw new Error("A Premiere project became active.");
    project = await ppro.Project.createProject(scratch);
    if (!project || project.path !== scratch) throw new Error("Could not create the exact scratch project.");
    imported = await importOriginals({
      schema: "anynle.premiere-import/1",
      request_id: request.id,
      scratch_path: scratch,
      scratch_guid: project.guid.toString(),
      originals: request.originals,
    }, folder);
  } catch (failure) {
    error = String(failure.message ?? failure);
  } finally {
    if (project && [scratch, donor].includes(project.path)) {
      try {
        const options = new ppro.CloseProjectOptions().setPromptIfDirty(false).setSaveWorkspace(false);
        if (!await project.close(options)) throw new Error("Close did not complete.");
      } catch (failure) {
        error = "Could not close the Any NLE scratch project. " + String(failure.message ?? failure);
      }
    }
  }
  await record(folder, "result.json", error
    ? { id: request.id, error }
    : { id: request.id, files: imported.files });
}

async function start() {
  const data = await fs.getDataFolder();
  const mailbox = (await entries(data)).get("worker-v1") ?? await data.createFolder("worker-v1");
  if (!mailbox.isFolder) throw new Error("Worker mailbox is occupied.");
  for (;;) {
    try {
      const state = await hasOpenProject() ? "project_open" : "ready";
      await record(mailbox, "status.json", { state, timestamp: Date.now() }, true);
      for (const [name, folder] of await entries(mailbox)) {
        if (!folder.isFolder || !name.startsWith(prefix) || !guid.test(name.slice(prefix.length))) continue;
        let request;
        try { request = await readJob(folder); } catch (_) { continue; }
        if (!request) continue;
        await record(mailbox, "status.json", { state: "busy", timestamp: Date.now() }, true);
        await execute(folder, request);
        break;
      }
    } catch (failure) {
      console.log("Any NLE media helper:", String(failure.message ?? failure));
    }
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
}

void start().catch((failure) => console.log("Any NLE media helper:", String(failure.message ?? failure)));
