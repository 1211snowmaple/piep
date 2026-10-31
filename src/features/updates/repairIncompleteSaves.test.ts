import { act, renderHook } from "@testing-library/react";
import { createElement } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ jobs:vi.fn(),credentials:vi.fn(),candidates:vi.fn(),works:vi.fn(),read:vi.fn(),start:vi.fn(),getJob:vi.fn(),listen:vi.fn() }));
vi.mock("@/services/dbApi", () => ({ getDownloads:api.works,readFileContent:api.read,isTauriRuntime:()=>true }));
vi.mock("@/services/updateJobApi", () => ({ listUpdateJobsCommand:api.jobs,getUpdateJobCredentials:api.credentials,listDeferredUpdateCandidatesCommand:api.candidates,startSaveJobCommand:api.start,getUpdateJobCommand:api.getJob }));
vi.mock("@/services/eventBus",()=>({subscribeTauriEvent:api.listen}));
import { REPAIR_BATCH_SIZE, startIncompleteSaveRepair, useIncompleteSaveRepair } from "./repairIncompleteSaves";

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  api.jobs.mockResolvedValue([]);
  api.credentials.mockResolvedValue({ pixivRefreshToken:"test-token",pixivCookie:"session",pixivUserAgent:"ua" });
  api.candidates.mockResolvedValue([{ source:"pixiv",sourceId:"1",downloadId:1,status:"held",payloadJson:'{"holdKind":"incomplete"}' }]);
  api.works.mockResolvedValue([{ id:1,sourceId:"1",title:"作品",jsonPath:"C:/library/v1/original.json",originalJsonPath:null,excerpt:null }]);
  api.read.mockResolvedValue('{"detail":{"caption":"","tags":[]},"text":"saved body"}');
  api.start.mockResolvedValue({ jobId:"repair-job",status:"running" });
});

it("queues missing details through the normal save job and does not repeat accepted work", async () => {
  expect((await startIncompleteSaveRepair()).job?.jobId).toBe("repair-job");
  expect(api.start).toHaveBeenCalledWith([{source:"pixiv",sourceId:"1",title:"作品"}],false,{pixivRefreshToken:"test-token",pixivCookie:"session",pixivUserAgent:"ua"});
  expect(await startIncompleteSaveRepair()).toEqual({ waiting:false });
  expect(api.start).toHaveBeenCalledOnce();
});

it("retries the generation which was blocked before native acquisition", async () => {
  localStorage.setItem("piep.incomplete-save-repair.v1", JSON.stringify(["pixiv:1"]));
  expect((await startIncompleteSaveRepair()).job?.jobId).toBe("repair-job");
  expect(await startIncompleteSaveRepair()).toEqual({waiting:false});
  expect(api.start).toHaveBeenCalledOnce();
});

it("repairs missing illustrations even when the caption and tags are already saved", async () => {
  api.read.mockResolvedValue('{"detail":{"caption":"","tags":[{"name":"tag"}]}}');
  expect((await startIncompleteSaveRepair()).job?.jobId).toBe("repair-job");
  expect(api.start).toHaveBeenCalledOnce();
});

it("waits for an existing download job instead of adding background traffic", async () => {
  api.jobs.mockResolvedValue([{status:"running"}]);
  expect(await startIncompleteSaveRepair()).toEqual({ waiting:true });
  expect(api.credentials).not.toHaveBeenCalled();
  expect(api.start).not.toHaveBeenCalled();
});

it("waits for a connection without marking the saved work as attempted", async () => {
  api.credentials.mockResolvedValue({pixivRefreshToken:null});
  expect(await startIncompleteSaveRepair()).toEqual({waiting:true});
  expect(localStorage.length).toBe(0);
  expect(api.start).not.toHaveBeenCalled();
});

it("does not queue a repair after its owner has unmounted", async () => {
  expect(await startIncompleteSaveRepair(()=>false)).toEqual({waiting:false});
  expect(api.start).not.toHaveBeenCalled();
  expect(localStorage.length).toBe(0);
});

it("coalesces concurrent startup requests into one durable job", async () => {
  const [a,b]=await Promise.all([startIncompleteSaveRepair(),startIncompleteSaveRepair()]);
  expect(a.job?.jobId).toBe(b.job?.jobId);
  expect(api.start).toHaveBeenCalledOnce();
});

it("works through a large backlog in short jobs with a pause between them",async()=>{
  vi.useFakeTimers();
  api.listen.mockImplementation(()=>()=>undefined);
  const count=REPAIR_BATCH_SIZE+3;
  api.candidates.mockResolvedValue(Array.from({length:count},(_,index)=>({source:"pixiv",sourceId:String(index),downloadId:index,status:"held",payloadJson:'{"holdKind":"incomplete"}'})));
  api.works.mockResolvedValue(Array.from({length:count},(_,index)=>({id:index,sourceId:String(index),title:`作品${index}`})));
  api.getJob.mockResolvedValue({jobId:"repair-job",status:"completed"});
  const client=new QueryClient();
  const hook=renderHook(()=>useIncompleteSaveRepair(),{wrapper:({children})=>createElement(QueryClientProvider,{client},children)});
  try {
    await act(()=>vi.advanceTimersByTimeAsync(5_000));
    expect(api.start).toHaveBeenCalledOnce();
    expect(api.start.mock.calls[0][0]).toHaveLength(REPAIR_BATCH_SIZE);
    await act(()=>vi.advanceTimersByTimeAsync(2_000));
    await act(()=>vi.advanceTimersByTimeAsync(59_000));
    expect(api.start).toHaveBeenCalledOnce();
    await act(()=>vi.advanceTimersByTimeAsync(1_000));
    expect(api.start).toHaveBeenCalledTimes(2);
    expect(api.start.mock.calls[1][0]).toHaveLength(3);
  } finally { hook.unmount();vi.useRealTimers(); }
});

it("picks up newly audited placeholders after the initial startup check finished",async()=>{
  vi.useFakeTimers();
  let onAudit!:(event:{payload:number[]})=>void;
  api.listen.mockImplementation((_name,handler)=>{onAudit=handler;return()=>undefined;});
  api.getJob.mockResolvedValue({jobId:"repair-job",status:"completed"});
  const candidates=await api.candidates();
  api.candidates.mockResolvedValue([]);
  const client=new QueryClient();
  const hook=renderHook(()=>useIncompleteSaveRepair(),{wrapper:({children})=>createElement(QueryClientProvider,{client},children)});
  try {
    await act(()=>vi.advanceTimersByTimeAsync(5_000));
    expect(api.start).not.toHaveBeenCalled();
    api.candidates.mockResolvedValue(candidates);
    act(()=>onAudit({payload:[1]}));
    await act(()=>vi.advanceTimersByTimeAsync(1_000));
    expect(api.start).toHaveBeenCalledOnce();
    act(()=>onAudit({payload:[1]}));
    await act(()=>vi.advanceTimersByTimeAsync(4_000));
    expect(api.start).toHaveBeenCalledOnce();
  } finally { hook.unmount();vi.useRealTimers(); }
});
