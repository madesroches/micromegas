/// Estimate quantiles based on a histogram
pub mod quantile;

/// Histogram data structures and aggregate function
pub mod histogram_udaf;

/// Merge a column of histograms of the same shape
pub mod sum_histograms_udaf;

/// Histogram accumulation
pub mod accumulator;

/// Get the count & sum of the values in the histogram
pub mod accessors;

/// Compute variance from running sum and sum of squares in the histogram
pub mod variance;
