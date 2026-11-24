class PatientFetcher < Fetcher
  %w[fetch store].each { |m| define_method("#{m}") { |*a| a } }
end
