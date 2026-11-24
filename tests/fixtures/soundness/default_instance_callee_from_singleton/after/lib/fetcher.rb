class Fetcher
  def fetch(key, retries: 3)
    [key, retries]
  end

  def self.go(x)
    fetch(x)
  end
end
