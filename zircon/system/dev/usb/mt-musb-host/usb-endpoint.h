// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include "usb-transaction.h"

#include <atomic>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <lib/mmio/mmio.h>
#include <lib/sync/completion.h>
#include <memory>
#include <threads.h>
#include <usb/request-cpp.h>
#include <zircon/hw/usb.h>
#include <zircon/thread_annotations.h>
#include <zircon/types.h>

namespace mt_usb_hci {

// The maximum single-buffered endpoint FIFO size.
constexpr uint32_t kFifoMaxSize = 4096;

// An Endpoint cultivates a queue of outstanding usb requests and asynchronously services them in
// serial-FIFO order.
class Endpoint {
public:
    virtual ~Endpoint() = default;

    // Advance processing of the current request which may optionally be the result of servicing a
    // hardware IRQ event (in which case interrupt should be set to true).
    virtual void Advance(bool interrupt) = 0;

    // Enqueue a new request for processing.
    virtual zx_status_t QueueRequest(usb::UnownedRequest<> req) = 0;

    // Start the request processing thread.
    virtual zx_status_t StartQueueThread() = 0;

    // Clear and cancel all currently pending requests from the queue.
    virtual zx_status_t CancelAll() = 0;

    // Return this endpoint's maximum packet transfer size (i.e. wMaxPacketSize).
    virtual size_t GetMaxTransferSize() = 0;

    // Halt endpoint request processing.  All outstanding requests will result in a
    // ZX_ERR_IO_NOT_PRESENT status, and the queue thread will be shut down.
    virtual zx_status_t Halt() = 0;

    // Block and wait until this endpoint is fully halted.
    virtual void Join() = 0;
};

// A TransactionEndpoint is an Endpoint which dispatches requests to a Transaction for processing.
// The templated type here is a Transaction-type which the endpoint uses to manage the lifetime of a
// USB transfer.
template <typename T>
class TransactionEndpoint : public Endpoint {
public:
    static_assert(std::is_base_of<Transaction, T>::value, "T must be derived from a Transaction");

    TransactionEndpoint(ddk::MmioView usb, uint8_t faddr,
                        const usb_endpoint_descriptor_t& descriptor)
        : usb_(usb),
          faddr_(faddr),
          max_pkt_sz_(usb_ep_max_packet(&descriptor)),
          descriptor_(descriptor),
          halted_(false) {}

    ~TransactionEndpoint() = default;

    void Advance(bool interrupt) override { transaction_->Advance(interrupt); }
    zx_status_t QueueRequest(usb::UnownedRequest<> req) override;
    zx_status_t StartQueueThread() override;
    zx_status_t CancelAll() override;
    size_t GetMaxTransferSize() override ;
    zx_status_t Halt() override;
    void Join() override { __UNUSED auto _ = sync_completion_wait(&complete_, ZX_TIME_INFINITE); }

protected:
    // The USB register mmio.
    ddk::MmioView usb_;

    // A transaction type used by this endpoint.
    std::unique_ptr<T> transaction_;

    // The id of the device this endpoint is associated with.
    uint8_t faddr_;

    // The maximum usb packet size for this transaction.
    size_t max_pkt_sz_;

private:
    // Dispatch and process a request transaction.  This method blocks until the transaction is
    // complete.
    zx_status_t DispatchRequest(usb::UnownedRequest<> req);

    // Queue thread which services enqueued requests in serial FIFO order.
    int QueueThread();

    // The queue of pending usb::UnownedRequests ready to be dispatched.  Requests are dispatched
    // and processed in FIFO-order.
    usb::UnownedRequestQueue<> pending_ TA_GUARDED(pending_lock_);

    // Queue dispatch thread.
    thrd_t pending_thread_;

    // Queue dispatch condition and associated mutex.
    fbl::Mutex pending_lock_;
    fbl::ConditionVariable pending_cond_ TA_GUARDED(pending_lock_);

    // The enumerated endpoint descriptor describing the behavior of this endpoint.
    usb_endpoint_descriptor_t descriptor_;

    // True if this endpoint has been halted.
    std::atomic_bool halted_;

    // A completion that is signaled upon completing the halt process.
    sync_completion_t complete_;
};

// A ControlEndpoint is an Endpoint dispatching control-type transactions.
class ControlEndpoint : public TransactionEndpoint<Control> {
public:
    // Note that initially all enumeration control transactions are performed on the default
    // control-pipe address of 0 using the spec. default maximum packet size of 8 bytes (encoded in
    // this type's static descriptor).  During enumeration, these values will be updated to their
    // final configured values.
    explicit ControlEndpoint(ddk::MmioView usb) : TransactionEndpoint(usb, 0, descriptor_) {}

    // Read the device descriptor (used only for enumeration).  Note that a successful
    // GET_DESCRIPTOR transaction will result in max_pkt_sz_ being updated with the bMaxPacketSize0
    // as returned by the device.
    zx_status_t GetDeviceDescriptor(usb_device_descriptor_t* out);

    // Set the USB function address for the device this endpoint is associated with (used only for
    // enumeration).  Note that a successful SET_ADDRESS transaction will result in faddr_  being
    // updated with the configured address.
    zx_status_t SetAddress(uint8_t addr);

private:
    // A minimal descriptor describing only the initial desired ControlEndpoint properties.
    static constexpr usb_endpoint_descriptor_t descriptor_ = {
        0,            // .bLength
        0,            // .bDescriptorType
        0,            // .bEndpointAddress
        0,            // .bmAttributes
        htole16(0x8), // .wMaxPacketSize (the only piece of data consumed)
        0,            // .bInterval
    };
};

// A BulkEndpoint is an Endpoint dispatching bulk-type transactions.
class BulkEndpoint : public TransactionEndpoint<Bulk> {
public:
    BulkEndpoint(ddk::MmioView usb, uint8_t faddr, const usb_endpoint_descriptor_t& descriptor)
        : TransactionEndpoint(usb, faddr, descriptor) {}
};

// An InterruptEndpoint is an Endpoint dispatching to interrupt-type transactions.
class InterruptEndpoint : public TransactionEndpoint<Interrupt> {
public:
    InterruptEndpoint(ddk::MmioView usb, uint8_t faddr,
                      const usb_endpoint_descriptor_t& descriptor)
        : TransactionEndpoint(usb, faddr, descriptor) {}
};

} // namespace mt_usb_hci
