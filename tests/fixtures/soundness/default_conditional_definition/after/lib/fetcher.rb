class Fetcher
  if ENV['PATIENT']
    def fetch(key, retries: 3)
      [key, retries]
    end
  end

  def go(x)
    fetch(x)
  end
end
