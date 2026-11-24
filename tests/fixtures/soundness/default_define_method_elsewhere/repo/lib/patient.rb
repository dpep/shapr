class PatientFetcher < Fetcher
  define_method(:fetch) { |key, retries: 10| [key, retries] }
end
