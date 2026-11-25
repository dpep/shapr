RSpec.describe Config do
  it "opts in" do
    model = Class.new(Base) { polyid }
    expect(model.attribute).to eq("uuid")
  end
end
