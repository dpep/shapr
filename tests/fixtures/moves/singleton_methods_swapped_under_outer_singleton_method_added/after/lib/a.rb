class Gauge
  def self.singleton_method_added(name)
    (@seen ||= []) << name
  end

  class << self
    def high
      limit + 1
    end

    def low
      limit - 1
    end
  end
end
