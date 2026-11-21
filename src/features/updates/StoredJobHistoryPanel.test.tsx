import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import type { StoredJobHistory } from "@/services/jobHistoryApi";

const mocks=vi.hoisted(()=>({history:vi.fn(),items:vi.fn(),logs:vi.fn(),hidden:vi.fn(),confirm:vi.fn(),notify:vi.fn(),status:{state:"ready",path:"x"} as {state:string;path?:string}}));
vi.mock("@/features/library/shelfV2",()=>({useLibraryV2Status:()=>({data:mocks.status})}));
vi.mock("@/services/jobHistoryApi",async(importOriginal)=>({...await importOriginal<typeof import("@/services/jobHistoryApi")>(),storedJobHistory:mocks.history,storedJobItems:mocks.items,storedJobLogs:mocks.logs,setStoredJobHidden:mocks.hidden}));
vi.mock("@mantine/modals",()=>({modals:{openConfirmModal:mocks.confirm}}));
vi.mock("@mantine/notifications",()=>({notifications:{show:mocks.notify}}));
import { StoredJobHistoryPanel } from "./StoredJobHistoryPanel";

const id="019ad000-0000-7000-8000-000000000001";
const job:StoredJobHistory={id,origin:"legacy-update-v1",scope:"author",mode:"check_only",status:"completed",watch_saved:false,new_works_only:false,totals:100,processed:7,candidate_count:3,saved_count:1,error_count:2,active_label:null,started_at:1_700_000_000_000,updated_at:1_700_000_000_100,finished_at:null};
const reload=vi.fn(async()=>{});
beforeEach(()=>{vi.clearAllMocks();mocks.status={state:"ready",path:"x"};mocks.history.mockResolvedValue([job]);mocks.items.mockResolvedValue([{id:"item-uuid",sequence:40,title:"当時の題名",status:"held",error:"当時の保留理由",result_work:null}]);mocks.logs.mockResolvedValue([{id:"log-uuid",sequence:90,log_type:"future-type",message:"当時の原文",created_at:job.started_at}]);mocks.hidden.mockResolvedValue(1);});
function open(){render(<MantineProvider><QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}})}><AppRouter><StoredJobHistoryPanel runtime current={<div>現在の記録</div>} hasCurrent={false} onShelfReady={reload}/></AppRouter></QueryClientProvider></MantineProvider>);}

it("opens original counts, items and logs using the shelf UUID without starting a job",async()=>{
  open();await screen.findByText("以前の実行記録");expect(screen.queryByText("現在の記録")).not.toBeInTheDocument();
  expect(screen.getByText("7/100件 · 候補 3 · 保存 1 · エラー 2")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button",{name:/の以前の実行記録を開く/}));
  await screen.findByText("当時の題名");expect(mocks.items).toHaveBeenCalledWith(id,null);expect(screen.getByText("終了：日時の記録なし")).toBeInTheDocument();
  expect(screen.queryByRole("button",{name:"保存する"})).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("tab",{name:"ログ"}));await screen.findByText("当時の原文");expect(mocks.logs).toHaveBeenCalledWith(id,null);expect(reload).toHaveBeenCalledOnce();
});

it("pages history by UUID and item/log pages by recorded sequence",async()=>{
  const first=Array.from({length:20},(_,index)=>({...job,id:`job-uuid-${index}`,started_at:job.started_at-index}));
  mocks.history.mockImplementation(async(before)=>before===null?first:[]);
  mocks.items.mockImplementation(async(_job,after)=>after===null?Array.from({length:100},(_,index)=>({id:`item-uuid-${index}`,sequence:10+index*3,title:`項目${index}`,status:"done",result_work:null})):[]);
  mocks.logs.mockImplementation(async(_job,after)=>after===null?Array.from({length:100},(_,index)=>({id:`log-uuid-${index}`,sequence:20+index*5,message:`原文${index}`,log_type:"info",created_at:job.started_at})):[]);
  open();fireEvent.click(await screen.findByRole("button",{name:"以前の記録を読み込む"}));await waitFor(()=>expect(mocks.history).toHaveBeenCalledWith("job-uuid-19"));
  fireEvent.click(screen.getAllByRole("button",{name:/の以前の実行記録を開く/})[0]);
  fireEvent.click(await screen.findByRole("button",{name:"次の項目を読み込む"}));await waitFor(()=>expect(mocks.items).toHaveBeenCalledWith("job-uuid-0",307));
  fireEvent.click(screen.getByRole("tab",{name:"ログ"}));fireEvent.click(await screen.findByRole("button",{name:"次のログを読み込む"}));await waitFor(()=>expect(mocks.logs).toHaveBeenCalledWith("job-uuid-0",515));
});

it("confirms hiding one record and allows undo without changing the original record",async()=>{
  let hidden=false;mocks.history.mockImplementation(async()=>hidden?[]:[job]);mocks.hidden.mockImplementation(async(_job,value)=>{hidden=value;return 1;});
  open();fireEvent.click(await screen.findByRole("button",{name:/の以前の実行記録を一覧から外す/}));expect(mocks.hidden).not.toHaveBeenCalled();
  await mocks.confirm.mock.calls[0][0].onConfirm();await waitFor(()=>expect(mocks.hidden).toHaveBeenCalledWith(id,true));
  await waitFor(()=>expect(mocks.notify).toHaveBeenCalled());render(<MantineProvider>{mocks.notify.mock.calls[0][0].message}</MantineProvider>);
  fireEvent.click(screen.getByRole("button",{name:"元に戻す"}));await waitFor(()=>expect(mocks.hidden).toHaveBeenCalledWith(id,false));
});

it("keeps the existing history while the shelf is still opening",async()=>{
  mocks.status={state:"opening",path:"x"};open();expect(screen.getByText("現在の記録")).toBeInTheDocument();await Promise.resolve();expect(mocks.history).not.toHaveBeenCalled();
});
