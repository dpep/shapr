module Freezer
  def self.unsafe(x) = x.dup.freeze
end

class A
end
T ||= Freezer

class A
  def run(x)
    x << 1
  end
end
