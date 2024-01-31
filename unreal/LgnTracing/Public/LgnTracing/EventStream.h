#pragma once
//
//  LgnTracing/EventStream.h
//
#include <string>
#include <vector>
#include <map>

namespace LgnTracing
{
    template< typename EventBlockT, size_t BUFFER_PADDING >
    class EventStreamImpl
    {
    public:
        typedef EventBlockT EventBlock;
        typedef std::shared_ptr<EventBlockT> BlockPtr;
        
        EventStreamImpl( const std::wstring& processId,
                         const std::wstring& streamId,
                         const BlockPtr& block,
                         const std::vector<std::wstring>& tags )
            : ProcessId( processId )
            , StreamId( streamId )
            , Tags( tags)
        {
            assert( block->GetCapacity() > BUFFER_PADDING );
            FullThreshold = block->GetCapacity() - BUFFER_PADDING;
            CurrentBlock = block;
        }

        const std::wstring& GetProcessId()const
        {
            return ProcessId;
        }

        const std::wstring& GetStreamId()const
        {
            return StreamId;
        }

        const std::vector<std::wstring>& GetTags()const
        {
            return Tags;
        }

        const std::map<std::wstring,std::wstring>& GetProperties()const
        {
            return Properties;
        }

        void SetProperty( const std::wstring& name, const std::wstring& value )
        {
            Properties[name] = value;
        }

        void MarkFull()
        {
            FullThreshold = 0;
        }
    
        BlockPtr SwapBlocks( const BlockPtr& newBlock )
        {
            BlockPtr old = CurrentBlock;
            CurrentBlock = newBlock;
            assert( CurrentBlock->GetCapacity() > BUFFER_PADDING );
            FullThreshold = CurrentBlock->GetCapacity() - BUFFER_PADDING;
            return old;
        }

        EventBlockT& GetCurrentBlock()
        {
            return *CurrentBlock;
        }

        const EventBlockT& GetCurrentBlock()const
        {
            return *CurrentBlock;
        }
    
        bool IsFull() const
        {
            return CurrentBlock->GetSizeBytes() >= FullThreshold;
        }

    private:
        std::wstring ProcessId;
        std::wstring StreamId;
        BlockPtr CurrentBlock;
        size_t FullThreshold;
        std::vector<std::wstring> Tags;
        std::map<std::wstring,std::wstring> Properties;
    };

}

