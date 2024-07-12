#include "RunningAverage.h"

FRunningAverage::FRunningAverage(size_t Capacity, double InitialValues)
	: Buffer(Capacity)
	, Sum(Capacity * InitialValues)
{
	for (size_t i = 0; i < Capacity; ++i)
	{
		Buffer.Add(InitialValues);
	}
}

void FRunningAverage::Add(double Value)
{
	double Oldest = Buffer.PopFrontValue();
	Sum -= Oldest;
	Buffer.Add(Value);
	Sum += Value;
}

double FRunningAverage::Get() const
{
	return Sum / static_cast<double>(Buffer.Num());
}
