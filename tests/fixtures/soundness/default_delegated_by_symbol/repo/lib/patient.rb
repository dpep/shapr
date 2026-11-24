class PatientFetcher < Fetcher
  extend Forwardable
  def_delegators :@inner, :fetch
end
