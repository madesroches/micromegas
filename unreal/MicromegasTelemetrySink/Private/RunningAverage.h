#pragma once

#include "Containers/RingBuffer.h"

class FRunningAverage
{
public:
	FRunningAverage(size_t Capacity, double InitialValues);
	void Add(double value);
	double Get() const;

private:
	TRingBuffer<double> Buffer;
	double Sum;
};
